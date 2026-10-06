/** What the tests share: the repository's paths and the engine they may drive. */

import { existsSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** js/fx. */
export const packageRoot = resolve(fileURLToPath(new URL(".", import.meta.url)), "..");
/** The repository. */
export const repoRoot = resolve(packageRoot, "..", "..");

/**
 * The engine the integration tests drive: `GRIDA_FX_BIN`, else the repository's own build
 * (`target/debug`, then `target/release`), else none, and those tests are skipped (the js job of
 * CI builds no engine).
 */
export function testEngine(): string | null {
  const configured = process.env.GRIDA_FX_BIN;
  if (configured) {
    return resolve(configured);
  }
  for (const profile of ["debug", "release"]) {
    const path = join(repoRoot, "target", profile, "grida-fx");
    if (existsSync(path) && statSync(path).isFile()) {
      return path;
    }
  }
  return null;
}

/** A new empty folder, removed by `cleanup`. */
export function temporaryFolder(prefix: string): { path: string; cleanup(): void } {
  const path = mkdtempSync(join(tmpdir(), `grida-fx-${prefix}-`));
  return { path, cleanup: () => rmSync(path, { recursive: true, force: true }) };
}

/** The value as JSON reads it back: `-0` is `0`, `undefined` members are gone. */
export function asJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/** The timeout of a test that runs the engine, Node, tsc or npm: CI machines are slow. */
export const SLOW = 60_000;
