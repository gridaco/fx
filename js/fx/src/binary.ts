/**
 * Finding the engine, the `grida-fx` binary. The `grida-fx` command this package installs and
 * every function of the SDK find it the same way:
 *
 * 1. `GRIDA_FX_BIN`, when set and not empty: the path of a binary (relative to the working
 *    directory). It must be a file.
 * 2. The engine package for this machine (`@grida/fx-darwin-arm64`, …; see `platforms.ts`), an
 *    optional dependency of `@grida/fx`: its `bin/grida-fx`.
 *
 * Nothing else: the command never looks on `PATH`, where it would find itself.
 */

import { statSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { FxError } from "./errors.js";
import { platformFor, unsupportedReason } from "./platforms.js";

/** The environment variable naming the binary to use. */
export const BINARY_VARIABLE = "GRIDA_FX_BIN";

/** An environment, as `process.env` holds it. */
export type Environment = Readonly<Record<string, string | undefined>>;

/** What finding the engine depends on, so it can be checked without the machine. */
export interface EngineLookup {
  readonly env: Environment;
  /** `process.platform`. */
  readonly os: string;
  /** `process.arch`. */
  readonly cpu: string;
  /** Where a relative `GRIDA_FX_BIN` starts. */
  readonly cwd: string;
  /** The path of a package's `package.json`, as `require.resolve` finds it; throws when absent. */
  resolvePackageJson(name: string): string;
  /** Whether a path is a file. */
  isFile(path: string): boolean;
  /** Whether this Linux system's C library is musl rather than glibc (asked only on Linux). */
  isMusl(): boolean;
}

/**
 * The engine's path, as the module comment says; throws an {@link FxError} that says what to do
 * when there is none.
 */
export function findEngine(lookup: EngineLookup): string {
  const configured = lookup.env[BINARY_VARIABLE];
  if (configured) {
    const path = resolve(lookup.cwd, expandHome(configured));
    if (!lookup.isFile(path)) {
      throw new FxError(`${BINARY_VARIABLE} is ${configured}, which is not a file`);
    }
    return path;
  }
  const platform = platformFor(lookup.os, lookup.cpu);
  if (platform === null) {
    throw new FxError(
      `${unsupportedReason(lookup.os, lookup.cpu)} Set ${BINARY_VARIABLE} to a grida-fx binary ` +
        `to use one built for this machine.`,
    );
  }
  let packageJson: string | null = null;
  try {
    packageJson = lookup.resolvePackageJson(`${platform.package}/package.json`);
  } catch {
    packageJson = null;
  }
  if (packageJson !== null) {
    const engine = join(dirname(packageJson), "bin", "grida-fx");
    if (lookup.isFile(engine)) {
      return engine;
    }
    throw new FxError(
      `${platform.package} is installed without its engine (${engine} is missing): reinstall ` +
        `@grida/fx, or set ${BINARY_VARIABLE} to a grida-fx binary`,
    );
  }
  if (platform.libc === "glibc" && lookup.isMusl()) {
    throw new FxError(
      `the preview of grida-fx needs glibc 2.28 or later on Linux, and this system's C library ` +
        `is musl (Alpine Linux, for one), so npm left out ${platform.package}. Use a glibc-based ` +
        `system or image (Debian, Ubuntu, …), or set ${BINARY_VARIABLE} to a grida-fx binary.`,
    );
  }
  throw new FxError(
    `the engine package ${platform.package} is not installed. npm installs it with @grida/fx, ` +
      `as an optional dependency, on ${platform.label}; it is left out when optional ` +
      `dependencies are omitted (--omit=optional, --no-optional, or a lockfile made without ` +
      `them). Reinstall @grida/fx with optional dependencies, or set ${BINARY_VARIABLE} to a ` +
      `grida-fx binary.`,
  );
}

/** Options of {@link binary}. */
export interface BinaryOptions {
  /** Variables over `process.env` (a variable given `undefined` is removed). */
  readonly env?: Environment;
  /** Where a relative `GRIDA_FX_BIN` starts (default: the process's working directory). */
  readonly cwd?: string;
}

/** The `grida-fx` binary the SDK drives, found as the module comment says. */
export function binary(options: BinaryOptions = {}): string {
  return findEngine(machineLookup(mergeEnvironment(options.env), options.cwd ?? process.cwd()));
}

/** The lookup of this machine and this installation of `@grida/fx`. */
export function machineLookup(env: Environment, cwd: string): EngineLookup {
  const require = createRequire(import.meta.url);
  return {
    env,
    os: process.platform,
    cpu: process.arch,
    cwd,
    resolvePackageJson: (name) => require.resolve(name),
    isFile,
    isMusl,
  };
}

/** `process.env` with `overrides` over it; a variable given `undefined` is removed. */
export function mergeEnvironment(overrides: Environment | undefined): Record<string, string> {
  const merged: Record<string, string> = {};
  for (const [name, value] of Object.entries(process.env)) {
    if (value !== undefined) {
      merged[name] = value;
    }
  }
  for (const [name, value] of Object.entries(overrides ?? {})) {
    if (value === undefined) {
      delete merged[name];
    } else {
      merged[name] = value;
    }
  }
  return merged;
}

function isFile(path: string): boolean {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

/** Whether the running Node was built against musl: its report names no glibc version. */
function isMusl(): boolean {
  try {
    const report = process.report?.getReport() as { header?: { glibcVersionRuntime?: string } };
    return report?.header !== undefined && report.header.glibcVersionRuntime === undefined;
  } catch {
    return false;
  }
}

function expandHome(path: string): string {
  return path === "~" || path.startsWith("~/") ? homedir() + path.slice(1) : path;
}
