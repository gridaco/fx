/**
 * The platforms the preview ships the engine for, one optional npm package each (as esbuild
 * does). `@grida/fx` depends on all four as optional dependencies; npm installs the one whose
 * `os`, `cpu` (and on Linux `libc`) fields match the machine. `tools/build_npm.mjs` reads this
 * table to make those packages, so it is the one list of them.
 *
 * Windows is not in the preview: the engine's runner relies on Unix process groups and signals.
 */

/** One platform the engine is built for. */
export interface Platform {
  /** The Rust target triple the binary is built for. */
  readonly triple: string;
  /** The npm package that carries the binary, at `bin/grida-fx`. */
  readonly package: string;
  /** `process.platform` on that machine, and the package's `os` field. */
  readonly os: "darwin" | "linux";
  /** `process.arch` on that machine, and the package's `cpu` field. */
  readonly cpu: "arm64" | "x64";
  /** The package's `libc` field, for Linux: the binary links glibc (2.28 or later). */
  readonly libc: "glibc" | null;
  /** For people: what the platform is. */
  readonly label: string;
}

/** Every platform of the preview. */
export const platforms: readonly Platform[] = [
  {
    triple: "aarch64-apple-darwin",
    package: "@grida/fx-darwin-arm64",
    os: "darwin",
    cpu: "arm64",
    libc: null,
    label: "macOS on Apple silicon (arm64)",
  },
  {
    triple: "x86_64-apple-darwin",
    package: "@grida/fx-darwin-x64",
    os: "darwin",
    cpu: "x64",
    libc: null,
    label: "macOS on Intel (x64)",
  },
  {
    triple: "x86_64-unknown-linux-gnu",
    package: "@grida/fx-linux-x64-gnu",
    os: "linux",
    cpu: "x64",
    libc: "glibc",
    label: "Linux on x64 with glibc 2.28 or later",
  },
  {
    triple: "aarch64-unknown-linux-gnu",
    package: "@grida/fx-linux-arm64-gnu",
    os: "linux",
    cpu: "arm64",
    libc: "glibc",
    label: "Linux on arm64 with glibc 2.28 or later",
  },
];

/** The platform for `process.platform` and `process.arch`, or `null` when the preview has none. */
export function platformFor(os: string, cpu: string): Platform | null {
  return platforms.find((platform) => platform.os === os && platform.cpu === cpu) ?? null;
}

/** The platform built for a Rust target triple, or `null`. */
export function platformForTriple(triple: string): Platform | null {
  return platforms.find((platform) => platform.triple === triple) ?? null;
}

/** Why the preview has no engine for this machine, in a sentence. */
export function unsupportedReason(os: string, cpu: string): string {
  const supported = platforms.map((platform) => platform.label).join("; ");
  const machine = `${os} on ${cpu}`;
  if (os === "win32") {
    return (
      `the preview of grida-fx does not run on Windows (the engine's runner uses Unix process ` +
      `groups and signals). It runs on: ${supported}. On Windows, use WSL.`
    );
  }
  return `the preview of grida-fx has no engine for ${machine}. It runs on: ${supported}.`;
}
