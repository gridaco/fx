import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { type EngineLookup, findEngine } from "../src/binary.js";
import { FxError } from "../src/errors.js";
import { platformFor, platformForTriple, platforms } from "../src/platforms.js";
import { packageRoot } from "./support.js";

/** A machine where `files` exist and `packages` are installed (name → package.json path). */
function lookup(
  overrides: Partial<EngineLookup> & { files?: string[]; packages?: Record<string, string> },
): EngineLookup {
  const files = new Set(overrides.files ?? []);
  const packages = overrides.packages ?? {};
  return {
    env: overrides.env ?? {},
    os: overrides.os ?? "darwin",
    cpu: overrides.cpu ?? "arm64",
    cwd: overrides.cwd ?? "/work",
    resolvePackageJson:
      overrides.resolvePackageJson ??
      ((name) => {
        const found = packages[name];
        if (found === undefined) {
          throw Object.assign(new Error(`Cannot find module '${name}'`), {
            code: "MODULE_NOT_FOUND",
          });
        }
        return found;
      }),
    isFile: overrides.isFile ?? ((path) => files.has(path)),
    isMusl: overrides.isMusl ?? (() => false),
  };
}

function failure(machine: EngineLookup): FxError {
  try {
    findEngine(machine);
  } catch (error) {
    expect(error).toBeInstanceOf(FxError);
    return error as FxError;
  }
  throw new Error("an engine was found");
}

const installed = {
  "@grida/fx-darwin-arm64/package.json": "/work/node_modules/@grida/fx-darwin-arm64/package.json",
};
const packagedEngine = "/work/node_modules/@grida/fx-darwin-arm64/bin/grida-fx";

describe("finding the engine", () => {
  test("GRIDA_FX_BIN comes first, relative to the working directory", () => {
    const machine = lookup({
      env: { GRIDA_FX_BIN: "bin/grida-fx" },
      files: ["/work/bin/grida-fx", packagedEngine],
      packages: installed,
    });
    expect(findEngine(machine)).toBe("/work/bin/grida-fx");
    expect(findEngine(lookup({ env: { GRIDA_FX_BIN: "/opt/fx" }, files: ["/opt/fx"] }))).toBe(
      "/opt/fx",
    );
  });

  test("a GRIDA_FX_BIN that is not a file is an error, not a fallback", () => {
    const error = failure(
      lookup({
        env: { GRIDA_FX_BIN: "/nowhere/grida-fx" },
        files: [packagedEngine],
        packages: installed,
      }),
    );
    expect(error.message).toBe("GRIDA_FX_BIN is /nowhere/grida-fx, which is not a file");
    expect(error.exitCode).toBeNull();
  });

  test("an empty GRIDA_FX_BIN is unset", () => {
    expect(
      findEngine(
        lookup({ env: { GRIDA_FX_BIN: "" }, files: [packagedEngine], packages: installed }),
      ),
    ).toBe(packagedEngine);
  });

  test("then the engine package for the machine", () => {
    expect(findEngine(lookup({ files: [packagedEngine], packages: installed }))).toBe(
      packagedEngine,
    );
    const linux = lookup({
      os: "linux",
      cpu: "x64",
      files: ["/n/@grida/fx-linux-x64-gnu/bin/grida-fx"],
      packages: {
        "@grida/fx-linux-x64-gnu/package.json": "/n/@grida/fx-linux-x64-gnu/package.json",
      },
    });
    expect(findEngine(linux)).toBe("/n/@grida/fx-linux-x64-gnu/bin/grida-fx");
  });

  test("a missing engine package says how to get it", () => {
    const error = failure(lookup({}));
    expect(error.message).toContain("the engine package @grida/fx-darwin-arm64 is not installed");
    expect(error.message).toContain("--omit=optional");
    expect(error.message).toContain("GRIDA_FX_BIN");
  });

  test("an engine package without its binary says so", () => {
    expect(failure(lookup({ packages: installed })).message).toContain(
      "@grida/fx-darwin-arm64 is installed without its engine",
    );
  });

  test("musl Linux is named", () => {
    const error = failure(lookup({ os: "linux", cpu: "arm64", isMusl: () => true }));
    expect(error.message).toContain("needs glibc 2.28 or later");
    expect(error.message).toContain("musl");
  });

  test("Windows and other platforms are not in the preview", () => {
    const windows = failure(lookup({ os: "win32", cpu: "x64" }));
    expect(windows.message).toContain("does not run on Windows");
    expect(windows.message).toContain("WSL");
    expect(failure(lookup({ os: "linux", cpu: "ia32" })).message).toContain(
      "has no engine for linux on ia32",
    );
    expect(failure(lookup({ os: "freebsd", cpu: "x64" })).message).toContain(
      "has no engine for freebsd on x64",
    );
    // GRIDA_FX_BIN still works there.
    expect(
      findEngine(
        lookup({ os: "win32", cpu: "x64", env: { GRIDA_FX_BIN: "/fx.exe" }, files: ["/fx.exe"] }),
      ),
    ).toBe("/fx.exe");
  });
});

describe("platforms", () => {
  test("the four targets of the preview", () => {
    expect(
      platforms.map((platform) => [
        platform.triple,
        platform.package,
        platform.os,
        platform.cpu,
        platform.libc,
      ]),
    ).toEqual([
      ["aarch64-apple-darwin", "@grida/fx-darwin-arm64", "darwin", "arm64", null],
      ["x86_64-apple-darwin", "@grida/fx-darwin-x64", "darwin", "x64", null],
      ["x86_64-unknown-linux-gnu", "@grida/fx-linux-x64-gnu", "linux", "x64", "glibc"],
      ["aarch64-unknown-linux-gnu", "@grida/fx-linux-arm64-gnu", "linux", "arm64", "glibc"],
    ]);
    expect(platformFor("darwin", "x64")?.triple).toBe("x86_64-apple-darwin");
    expect(platformFor("linux", "arm64")?.package).toBe("@grida/fx-linux-arm64-gnu");
    expect(platformFor("win32", "x64")).toBeNull();
    expect(platformForTriple("aarch64-unknown-linux-gnu")?.cpu).toBe("arm64");
    expect(platformForTriple("x86_64-pc-windows-msvc")).toBeNull();
  });

  test("@grida/fx depends on each engine package, optionally, at its own version", () => {
    const manifest = JSON.parse(readFileSync(join(packageRoot, "package.json"), "utf8"));
    const wanted = Object.fromEntries(
      platforms.map((platform) => [platform.package, manifest.version]),
    );
    expect(manifest.optionalDependencies).toEqual(wanted);
    expect(manifest.dependencies).toBeUndefined();
    expect(manifest.bin).toEqual({ "grida-fx": "bin/grida-fx.js" });
    expect(manifest.engines).toEqual({ node: ">=18" });
  });
});
