// tools/build_npm.mjs with stand-in engines: ELF headers of a Linux target this machine is not, so
// nothing is run. Skipped without Node or without npm.

import { afterAll, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { platforms } from "../src/platforms.js";
import { packageRoot, repoRoot, SLOW, temporaryFolder } from "./support.js";

const tools =
  spawnSync("node", ["--version"]).status === 0 && spawnSync("npm", ["--version"]).status === 0;
const manifest = JSON.parse(readFileSync(join(packageRoot, "package.json"), "utf8"));
const version: string = manifest.version;

/** A Linux platform this machine is not, so its stand-in is never run. */
const linux = platforms.find(
  (platform) =>
    platform.os === "linux" && !(process.platform === "linux" && process.arch === platform.cpu),
);

/** The first 64 bytes of a 64-bit little-endian ELF executable for `cpu`, then `tail`. */
function elf(cpu: "x64" | "arm64", tail = ""): Buffer {
  const header = Buffer.alloc(64);
  header.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1], 0);
  header.writeUInt16LE(2, 16);
  header.writeUInt16LE(cpu === "x64" ? 62 : 183, 18);
  return Buffer.concat([header, Buffer.from(tail, "latin1")]);
}

/** An ELF executable for `cpu` with a section table: `.dynstr` holds `dynstr`, `.rodata` holds
 * `rodata`. */
function elfWithSections(cpu: "x64" | "arm64", dynstr: string, rodata: string): Buffer {
  const shstrtab = Buffer.from("\0.shstrtab\0.dynstr\0.rodata\0", "latin1");
  const bodies = [shstrtab, Buffer.from(`\0${dynstr}\0`, "latin1"), Buffer.from(rodata, "latin1")];
  const names = [1, 11, 19];
  let at = 64;
  const offsets = bodies.map((body) => {
    const offset = at;
    at += body.length;
    return offset;
  });
  const table = Buffer.alloc(64 * 4);
  bodies.forEach((body, index) => {
    const entry = 64 * (index + 1);
    table.writeUInt32LE(names[index] as number, entry);
    table.writeBigUInt64LE(BigInt(offsets[index] as number), entry + 24);
    table.writeBigUInt64LE(BigInt(body.length), entry + 32);
  });
  const header = elf(cpu);
  header.writeBigUInt64LE(BigInt(at), 0x28);
  header.writeUInt16LE(64, 0x3a);
  header.writeUInt16LE(4, 0x3c);
  header.writeUInt16LE(1, 0x3e);
  return Buffer.concat([header, ...bodies, table]);
}

describe.skipIf(!tools || linux === undefined)("tools/build_npm.mjs", () => {
  const folder = temporaryFolder("build-npm");
  afterAll(() => folder.cleanup());
  const platform = linux as (typeof platforms)[number];
  const build = (...args: string[]) =>
    spawnSync("node", [join(repoRoot, "tools", "build_npm.mjs"), ...args], {
      cwd: folder.path,
      encoding: "utf8",
    });

  test(
    "@grida/fx and an engine package, packed",
    () => {
      const engine = join(folder.path, "engine");
      writeFileSync(engine, elf(platform.cpu, "\0GLIBC_2.17\0GLIBC_2.28\0GLIBC_PRIVATE\0"));
      const out = join(folder.path, "out");
      const done = build("--out", out, "--target", `${platform.triple}=${engine}`);
      expect(done.stderr).toBe("");
      expect(done.status).toBe(0);
      const name = platform.package.replace("@grida/", "");
      expect(done.stdout).toContain(`@grida/fx@${version}  `);
      expect(done.stdout).toContain(`${platform.package}@${version}  `);
      expect(statSync(join(out, `grida-fx-${version}.tgz`)).isFile()).toBe(true);
      expect(statSync(join(out, `grida-${name}-${version}.tgz`)).isFile()).toBe(true);

      const sdk = JSON.parse(readFileSync(join(out, "packages", "fx", "package.json"), "utf8"));
      expect(sdk.version).toBe(version);
      expect(readFileSync(join(out, "packages", "fx", "LICENSE"), "utf8")).toBe(
        readFileSync(join(repoRoot, "LICENSE"), "utf8"),
      );
      expect(sdk.scripts).toBeUndefined();
      expect(sdk.devDependencies).toBeUndefined();
      expect(sdk.optionalDependencies[platform.package]).toBe(version);
      expect(statSync(join(out, "packages", "fx", "bin", "grida-fx.js")).mode & 0o777).toBe(0o755);
      expect(statSync(join(out, "packages", "fx", "dist", "index.d.ts")).isFile()).toBe(true);

      const engineFolder = join(out, "packages", name);
      const package_ = JSON.parse(readFileSync(join(engineFolder, "package.json"), "utf8"));
      expect(package_).toMatchObject({
        name: platform.package,
        version,
        os: ["linux"],
        cpu: [platform.cpu],
        libc: ["glibc"],
        files: ["bin/grida-fx", "README.md", "LICENSE"],
        license: "Apache-2.0",
        homepage: "https://grida.co/fx",
        publishConfig: { access: "public", tag: "next" },
      });
      expect(package_.bin).toBeUndefined();
      expect(readFileSync(join(engineFolder, "LICENSE"), "utf8")).toBe(
        readFileSync(join(repoRoot, "LICENSE"), "utf8"),
      );
      expect(statSync(join(engineFolder, "bin", "grida-fx")).mode & 0o777).toBe(0o755);
      expect(readFileSync(join(engineFolder, "README.md"), "utf8")).toContain(
        "npm install @grida/fx@next",
      );
    },
    SLOW,
  );

  test(
    "an engine that does not fit its target is refused",
    () => {
      const out = join(folder.path, "refused");
      const other = platform.cpu === "x64" ? "arm64" : "x64";
      const wrongArch = join(folder.path, "wrong-arch");
      writeFileSync(wrongArch, elf(other));
      let done = build("--out", out, "--no-pack", "--target", `${platform.triple}=${wrongArch}`);
      expect(done.status).toBe(1);
      expect(done.stderr).toContain("an ELF executable for another architecture");

      const newGlibc = join(folder.path, "new-glibc");
      writeFileSync(newGlibc, elf(platform.cpu, "\0GLIBC_2.34\0"));
      done = build("--out", out, "--no-pack", "--target", `${platform.triple}=${newGlibc}`);
      expect(done.status).toBe(1);
      expect(done.stderr).toContain("needs glibc 2.34, newer than the 2.28");

      // Only the dynamic string table counts: other text naming a glibc is not a requirement.
      const sections = join(folder.path, "sections");
      writeFileSync(sections, elfWithSections(platform.cpu, "GLIBC_2.17", "GLIBC_2.99"));
      done = build("--out", out, "--no-pack", "--target", `${platform.triple}=${sections}`);
      expect(done.stderr).toBe("");
      expect(done.status).toBe(0);
      writeFileSync(sections, elfWithSections(platform.cpu, "GLIBC_2.34", "GLIBC_2.17"));
      done = build("--out", out, "--no-pack", "--target", `${platform.triple}=${sections}`);
      expect(done.status).toBe(1);
      expect(done.stderr).toContain("needs glibc 2.34");

      done = build("--out", out, "--no-pack", "--target", `aarch64-apple-darwin=${newGlibc}`);
      expect(done.status).toBe(1);
      expect(done.stderr).toContain("is not a thin 64-bit Mach-O executable");

      done = build("--out", out, "--no-pack", "--target", `x86_64-pc-windows-msvc=${newGlibc}`);
      expect(done.status).toBe(1);
      expect(done.stderr).toContain("is not a target of the preview");

      done = build("--out", repoRoot);
      expect(done.status).toBe(1);
      expect(done.stderr).toContain("would write over the repository's own files");

      done = build("--target", "no-binary");
      expect(done.stderr).toContain("write TRIPLE=BINARY");
    },
    SLOW,
  );
});
