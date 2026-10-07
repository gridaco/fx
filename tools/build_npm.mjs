#!/usr/bin/env node
// Builds the npm packages of a release (milestone 1, step 5): @grida/fx, and one engine package
// per built binary. Publishing is not done here (RELEASING.md).
//
//     node tools/build_npm.mjs --out dist/npm --target aarch64-apple-darwin=target/release/grida-fx
//
// - **Version.** The Cargo workspace's ([workspace.package] version), the one source of truth.
//   js/fx/package.json, its pins of the engine packages and the `version` constant of
//   js/fx/src/index.ts must state it already (tools/check_versions.py); a disagreement stops the
//   build rather than being rewritten here.
// - **@grida/fx** (`<out>/packages/fx/`): js/fx/package.json without its scripts and
//   devDependencies, README.md, bin/grida-fx.js (mode 755), and dist/ compiled from js/fx/src by
//   the TypeScript compiler js/fx has installed (`bun install --frozen-lockfile` in js/fx).
// - **An engine package per `--target <triple>=<binary>`** (`<out>/packages/fx-<os>-<cpu>…/`):
//   package.json with the `os`, `cpu` (and on Linux `libc`) fields npm installs it by, README.md,
//   and the binary at bin/grida-fx (mode 755). The targets and package names are the table in
//   js/fx/src/platforms.ts, read from the compiled dist. The binary must be a 64-bit Mach-O (macOS)
//   or little-endian ELF (Linux) of the target's architecture; a Linux binary must need no glibc
//   symbol version past 2.28; and when the target is this machine's, `<binary> --version` must
//   print `grida-fx <version>`.
// - **Tarballs.** Each folder is packed with `npm pack` into `<out>/` (`grida-fx-<version>.tgz`,
//   `grida-fx-darwin-arm64-<version>.tgz`, …), and each tarball's file list and modes are
//   checked. `--no-pack` stops after the folders.
//
// Node 18 or later, no dependencies. Exit status 1 with `build_npm: <reason>` when a package
// cannot be built as promised.

import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SDK = join(REPO, "js", "fx");
const SCOPE = "@grida/";
/** Linux engines link glibc 2.28 at most (cargo zigbuild --target <triple>.2.28). */
const GLIBC_BASELINE = [2, 28];

class BuildError extends Error {}

function fail(message) {
  throw new BuildError(message);
}

// ------------------------------------------------------------------------------------------------
// Arguments

function usage() {
  return [
    "usage: node tools/build_npm.mjs [--out DIR] [--target TRIPLE=BINARY]... [--no-pack]",
    "",
    "  --out DIR                 where the packages go (default: target/npm)",
    "  --target TRIPLE=BINARY    an engine package for TRIPLE carrying BINARY; repeatable",
    "  --no-pack                 assemble the package folders, without npm pack",
  ].join("\n");
}

function parseArguments(argv) {
  const options = { out: join(REPO, "target", "npm"), targets: [], pack: true };
  const value = (name, index) => {
    const given = argv[index + 1];
    if (given === undefined) {
      fail(`${name} needs a value\n${usage()}`);
    }
    return given;
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    const [name, inline] =
      argument.startsWith("--") && argument.includes("=")
        ? [argument.slice(0, argument.indexOf("=")), argument.slice(argument.indexOf("=") + 1)]
        : [argument, undefined];
    if (name === "--help" || name === "-h") {
      console.log(usage());
      process.exit(0);
    } else if (name === "--out") {
      options.out = resolve(inline ?? value(name, index));
      index += inline === undefined ? 1 : 0;
    } else if (name === "--target") {
      const pair = inline ?? value(name, index);
      index += inline === undefined ? 1 : 0;
      const at = pair.indexOf("=");
      if (at <= 0 || at === pair.length - 1) {
        fail(`--target ${pair}: write TRIPLE=BINARY`);
      }
      options.targets.push({ triple: pair.slice(0, at), binary: resolve(pair.slice(at + 1)) });
    } else if (name === "--no-pack") {
      options.pack = false;
    } else {
      fail(`unknown argument ${argument}\n${usage()}`);
    }
  }
  const triples = options.targets.map((target) => target.triple);
  const repeated = triples.find((triple, index) => triples.indexOf(triple) !== index);
  if (repeated !== undefined) {
    fail(`--target ${repeated} is given twice`);
  }
  const out = options.out;
  if (out === REPO || REPO.startsWith(out + sep) || out === SDK || out.startsWith(SDK + sep)) {
    fail(`--out ${out} would write over the repository's own files: choose another folder`);
  }
  return options;
}

// ------------------------------------------------------------------------------------------------
// The version

/** The Cargo workspace's version. */
function workspaceVersion() {
  const text = readFileSync(join(REPO, "Cargo.toml"), "utf8");
  const table = text.match(/^\[workspace\.package\][ \t]*\r?\n([\s\S]*?)(?=^\[|(?![\s\S]))/m);
  const version = table?.[1].match(/^version[ \t]*=[ \t]*"([^"]+)"/m)?.[1];
  if (version === undefined) {
    fail("Cargo.toml has no [workspace.package] version");
  }
  return version;
}

/** The SDK's manifest, checked against the version. */
function sdkManifest(version) {
  const manifest = JSON.parse(readFileSync(join(SDK, "package.json"), "utf8"));
  const problems = [];
  if (manifest.version !== version) {
    problems.push(`js/fx/package.json version is ${manifest.version}`);
  }
  for (const [name, wanted] of Object.entries(manifest.optionalDependencies ?? {})) {
    if (name.startsWith(`${SCOPE}fx-`) && wanted !== version) {
      problems.push(`js/fx/package.json pins ${name} to ${wanted}`);
    }
  }
  const index = readFileSync(join(SDK, "src", "index.ts"), "utf8");
  const stated = index.match(/export\s+const\s+version\s*=\s*["']([^"']*)["']/)?.[1];
  if (stated !== version) {
    problems.push(`js/fx/src/index.ts states version ${stated ?? "nothing"}`);
  }
  if (problems.length > 0) {
    fail(
      `the workspace version is ${version}, but ${problems.join("; ")}. ` +
        "Make them agree (python3 tools/check_versions.py names every place).",
    );
  }
  manifest.publishConfig = { access: "public", tag: version.includes("-") ? "next" : "latest" };
  return manifest;
}

// ------------------------------------------------------------------------------------------------
// @grida/fx

function buildSdk(manifest, version, folder) {
  const tsc = join(SDK, "node_modules", "typescript", "bin", "tsc");
  if (!existsSync(tsc)) {
    fail("TypeScript is not installed in js/fx: run `bun install --frozen-lockfile` there first");
  }
  rmSync(folder, { recursive: true, force: true });
  mkdirSync(join(folder, "bin"), { recursive: true });
  const compiled = spawnSync(
    process.execPath,
    [tsc, "-p", join(SDK, "tsconfig.build.json"), "--outDir", join(folder, "dist")],
    { cwd: SDK, encoding: "utf8" },
  );
  if (compiled.status !== 0) {
    fail(`tsc failed:\n${compiled.stdout}${compiled.stderr}`);
  }
  copyFileSync(join(SDK, "bin", "grida-fx.js"), join(folder, "bin", "grida-fx.js"));
  chmodSync(join(folder, "bin", "grida-fx.js"), 0o755);
  copyFileSync(join(SDK, "README.md"), join(folder, "README.md"));
  copyFileSync(join(SDK, "LICENSE"), join(folder, "LICENSE"));
  const published = { ...manifest, version };
  delete published.scripts;
  delete published.devDependencies;
  delete published.private;
  writeJson(join(folder, "package.json"), published);
}

// ------------------------------------------------------------------------------------------------
// Engine packages

/** The machine-code facts of `data` a target needs: its format and architecture. */
function checkBinary(data, platform, path) {
  const shown = relative(process.cwd(), path) || path;
  if (platform.os === "darwin") {
    const magic = data.length >= 8 ? data.readUInt32LE(0) : 0;
    if (magic !== 0xfeedfacf) {
      fail(`${shown} is not a thin 64-bit Mach-O executable, which ${platform.triple} needs`);
    }
    const cpu = { arm64: 0x0100000c, x64: 0x01000007 }[platform.cpu];
    if (data.readUInt32LE(4) !== cpu) {
      fail(`${shown} is a Mach-O executable for another architecture than ${platform.triple}`);
    }
    return;
  }
  const elf =
    data.length >= 20 && data.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]));
  if (!elf || data[4] !== 2 || data[5] !== 1) {
    fail(`${shown} is not a 64-bit little-endian ELF executable, which ${platform.triple} needs`);
  }
  const machine = { x64: 62, arm64: 183 }[platform.cpu];
  if (data.readUInt16LE(18) !== machine) {
    fail(`${shown} is an ELF executable for another architecture than ${platform.triple}`);
  }
  // The newest glibc symbol version its dynamic string table names: the oldest glibc it runs on.
  let newest = null;
  const names = dynamicStrings(data).toString("latin1");
  for (const match of names.matchAll(/GLIBC_(\d+)\.(\d+)(?:\.(\d+))?/g)) {
    const found = [Number(match[1]), Number(match[2])];
    if (newest === null || compare(found, newest) > 0) {
      newest = found;
    }
  }
  if (newest !== null && compare(newest, GLIBC_BASELINE) > 0) {
    fail(
      `${shown} needs glibc ${newest.join(".")}, newer than the ${GLIBC_BASELINE.join(".")} its ` +
        `package promises: build it with cargo zigbuild --target ${platform.triple}.${GLIBC_BASELINE.join(".")}`,
    );
  }
}

/** The bytes of an ELF file's `.dynstr` section, where the symbol versions it needs are named;
 * the whole file when it has no section table to find it by. */
function dynamicStrings(data) {
  if (data.length < 64) {
    return data;
  }
  const sectionsAt = Number(data.readBigUInt64LE(0x28));
  const entrySize = data.readUInt16LE(0x3a);
  const count = data.readUInt16LE(0x3c);
  const namesIndex = data.readUInt16LE(0x3e);
  const section = (index) => {
    const at = sectionsAt + index * entrySize;
    return {
      name: data.readUInt32LE(at),
      offset: Number(data.readBigUInt64LE(at + 24)),
      size: Number(data.readBigUInt64LE(at + 32)),
    };
  };
  if (sectionsAt === 0 || entrySize < 64 || sectionsAt + count * entrySize > data.length) {
    return data;
  }
  if (namesIndex >= count) {
    return data;
  }
  const names = section(namesIndex);
  for (let index = 0; index < count; index += 1) {
    const candidate = section(index);
    const at = names.offset + candidate.name;
    const name = data.subarray(at, data.indexOf(0, at)).toString("latin1");
    if (name === ".dynstr") {
      return data.subarray(candidate.offset, candidate.offset + candidate.size);
    }
  }
  return data;
}

function compare(a, b) {
  return a[0] - b[0] || a[1] - b[1];
}

/** Whether `platform` is this machine's, so its binary can run here. */
function runsHere(platform) {
  if (process.platform !== platform.os || process.arch !== platform.cpu) {
    return false;
  }
  if (platform.libc !== "glibc") {
    return true;
  }
  try {
    return process.report.getReport().header.glibcVersionRuntime !== undefined;
  } catch {
    return false;
  }
}

function buildEngine(platform, binary, version, manifest, folder) {
  if (!existsSync(binary) || !statSync(binary).isFile()) {
    fail(`${binary} is not a file`);
  }
  checkBinary(readFileSync(binary), platform, binary);
  if (runsHere(platform)) {
    const ran = spawnSync(binary, ["--version"], { encoding: "utf8" });
    const printed = (ran.stdout ?? "").trim();
    if (ran.status !== 0 || printed !== `grida-fx ${version}`) {
      fail(`${binary} --version printed ${JSON.stringify(printed)}, not "grida-fx ${version}"`);
    }
  }
  rmSync(folder, { recursive: true, force: true });
  mkdirSync(join(folder, "bin"), { recursive: true });
  copyFileSync(binary, join(folder, "bin", "grida-fx"));
  chmodSync(join(folder, "bin", "grida-fx"), 0o755);
  copyFileSync(join(REPO, "LICENSE"), join(folder, "LICENSE"));
  const package_ = {
    name: platform.package,
    version,
    description: `The grida-fx engine for ${platform.label}. @grida/fx installs it; use @grida/fx.`,
    homepage: manifest.homepage,
    bugs: manifest.bugs,
    repository: { ...manifest.repository, directory: "crates/grida-fx" },
    ...(manifest.license !== undefined ? { license: manifest.license } : {}),
    os: [platform.os],
    cpu: [platform.cpu],
    ...(platform.libc !== null ? { libc: [platform.libc] } : {}),
    files: ["bin/grida-fx", "README.md", "LICENSE"],
    preferUnplugged: true,
    publishConfig: manifest.publishConfig,
  };
  writeJson(join(folder, "package.json"), package_);
  writeFileSync(join(folder, "README.md"), engineReadme(platform, version));
}

function engineReadme(platform, version) {
  return [
    `# ${platform.package}`,
    "",
    `The \`grida-fx\` engine (version ${version}) for ${platform.label}, at \`bin/grida-fx\`.`,
    "",
    "This package is an optional dependency of [`@grida/fx`](https://www.npmjs.com/package/@grida/fx),",
    "and npm installs it only on a matching machine. Install `@grida/fx` instead:",
    "",
    "```sh",
    `npm install @grida/fx${version.includes("-") ? "@next" : ""}`,
    "npx grida-fx --help",
    "```",
    "",
    "Grida FX is a workflow engine for generative asset pipelines: https://github.com/gridaco/fx",
    "",
  ].join("\n");
}

// ------------------------------------------------------------------------------------------------
// Packing

/** Packs `folder` into `out` with npm, checks the tarball's files, and returns its file name. */
function pack(folder, out, expected) {
  const npm = process.platform === "win32" ? "npm.cmd" : "npm";
  const packed = spawnSync(
    npm,
    ["pack", folder, "--pack-destination", out, "--json", "--ignore-scripts"],
    { cwd: out, encoding: "utf8" },
  );
  if (packed.status !== 0) {
    fail(`npm pack ${folder} failed:\n${packed.stderr}`);
  }
  const [summary] = JSON.parse(packed.stdout);
  const files = new Map(summary.files.map((file) => [file.path, file.mode]));
  for (const [path, mode] of Object.entries(expected)) {
    if (!files.has(path)) {
      fail(`${summary.filename} has no ${path}`);
    }
    if (mode !== null && (files.get(path) & 0o777) !== mode) {
      fail(
        `${summary.filename} holds ${path} with mode ${(files.get(path) & 0o777).toString(8)}, not ${mode.toString(8)}`,
      );
    }
  }
  return summary;
}

// ------------------------------------------------------------------------------------------------

function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

function folderName(name) {
  return name.startsWith(SCOPE) ? name.slice(SCOPE.length) : name;
}

async function main(argv) {
  const options = parseArguments(argv);
  const version = workspaceVersion();
  const manifest = sdkManifest(version);
  const packages = join(options.out, "packages");
  mkdirSync(packages, { recursive: true });

  const sdkFolder = join(packages, folderName(manifest.name));
  buildSdk(manifest, version, sdkFolder);
  const { platformForTriple, platforms } = await import(
    pathToFileURL(join(sdkFolder, "dist", "platforms.js")).href
  );
  const wanted = Object.keys(manifest.optionalDependencies ?? {}).sort();
  const known = platforms.map((platform) => platform.package).sort();
  if (JSON.stringify(wanted) !== JSON.stringify(known)) {
    fail(
      `js/fx/package.json depends on ${wanted.join(", ")}, but js/fx/src/platforms.ts lists ` +
        known.join(", "),
    );
  }

  const built = [{ name: manifest.name, folder: sdkFolder, expected: sdkFiles() }];
  for (const target of options.targets) {
    const platform = platformForTriple(target.triple);
    if (platform === null) {
      fail(
        `--target ${target.triple} is not a target of the preview: ` +
          platforms.map((known) => known.triple).join(", "),
      );
    }
    const folder = join(packages, folderName(platform.package));
    buildEngine(platform, target.binary, version, manifest, folder);
    built.push({
      name: platform.package,
      folder,
      expected: { "package.json": null, "README.md": null, "LICENSE": null, "bin/grida-fx": 0o755 },
    });
  }
  const missing = platforms.filter(
    (platform) => !options.targets.some((target) => target.triple === platform.triple),
  );

  for (const { name, folder, expected } of built) {
    if (!options.pack) {
      console.log(`${name}@${version}  ${relative(process.cwd(), folder) || folder}`);
      continue;
    }
    const summary = pack(folder, options.out, expected);
    const tarball = join(options.out, summary.filename);
    console.log(
      `${name}@${version}  ${relative(process.cwd(), tarball) || basename(tarball)}  ` +
        `${summary.size} bytes  ${summary.entryCount} files  sha1 ${summary.shasum}`,
    );
  }
  if (missing.length > 0) {
    console.log(
      `not built here: ${missing.map((platform) => platform.package).join(", ")} ` +
        "(each comes from its own --target)",
    );
  }
}

/** What @grida/fx's tarball must hold: the manifest, the readme, the command, and each module of
 * the compiled SDK with its declarations. */
function sdkFiles() {
  const expected = { "package.json": null, "README.md": null, "LICENSE": null, "bin/grida-fx.js": 0o755 };
  for (const module of ["index", "cli", "binary", "platforms", "api"]) {
    expected[`dist/${module}.js`] = null;
    expected[`dist/${module}.d.ts`] = null;
  }
  return expected;
}

try {
  await main(process.argv.slice(2));
} catch (error) {
  if (error instanceof BuildError) {
    process.stderr.write(`build_npm: ${error.message}\n`);
    process.exit(1);
  }
  throw error;
}
