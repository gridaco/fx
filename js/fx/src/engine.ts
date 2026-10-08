/**
 * Running the engine and reading what it prints.
 *
 * The engine runs in the caller's process group, so a Ctrl-C at the terminal reaches it as it
 * reaches the caller, and it stops a run as it would on its own: do not also abort the call on
 * that Ctrl-C (a second interruption makes the engine end at once). An `AbortSignal` stops it from
 * code: a run gets one idempotent SIGTERM and retains ownership until engine cleanup ends, without
 * an automatic force deadline. Other commands retain their ten-second SIGKILL fallback.
 *
 * A call ends when the engine exits, not when the last holder of its pipes closes them: a process
 * a node body started may outlive the engine on purpose.
 */

import { spawn } from "node:child_process";
import { type Environment, findEngine, machineLookup, mergeEnvironment } from "./binary.js";
import { FxError } from "./errors.js";

/** Options every call takes. */
export interface CallOptions {
  /** The engine's working directory (default: the process's). Relative paths start here. */
  readonly cwd?: string;
  /** Variables over `process.env` for the engine (`undefined` removes one), e.g.
   * `GRIDA_FX_PYTHON`. `GRIDA_FX_BIN` here also chooses the binary. */
  readonly env?: Environment;
  /** Interrupts the engine once. A run waits for engine cleanup before rejecting with the
   * signal's reason; other commands retain a ten-second force fallback. */
  readonly signal?: AbortSignal;
}

/** How a call ended. */
export interface Exit {
  /** The exit status; null when a signal ended the engine. */
  readonly status: number | null;
  readonly signal: string | null;
  readonly stdout: string;
  readonly stderr: string;
  readonly args: readonly string[];
}

/** The exit status of an unreadable input or a usage mistake. */
export const USAGE_OR_ERROR = 2;
/** The exit status of an interrupted command. */
export const INTERRUPTED = 130;
/** The stop grace for non-run commands. Run cleanup has no automatic force deadline. */
const STOP_GRACE_MS = 10_000;
/** How long the pipes are still read once the engine has exited. */
const DRAIN_MS = 500;
/** How the engine prefixes an error on stderr. */
const ERROR_PREFIX = "grida-fx: ";

/** Runs the engine with `args` and collects what it prints. */
export async function call(args: readonly string[], options: CallOptions = {}): Promise<Exit> {
  const cwd = options.cwd ?? process.cwd();
  const env = mergeEnvironment(options.env);
  if (args[0] === "run") {
    env.GRIDA_FX_CANCEL_SOURCE = "sdk";
  }
  const engine = findEngine(machineLookup(env, cwd));
  options.signal?.throwIfAborted();
  return new Promise<Exit>((resolvePromise, reject) => {
    const child = spawn(engine, [...args], {
      cwd,
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
    child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
    let killTimer: NodeJS.Timeout | undefined;
    let interrupted = false;
    const onAbort = () => {
      if (!interrupted && child.exitCode === null && child.signalCode === null) {
        interrupted = true;
        child.kill(args[0] === "run" ? "SIGTERM" : "SIGINT");
        if (args[0] !== "run") {
          killTimer = setTimeout(() => child.kill("SIGKILL"), STOP_GRACE_MS);
        }
      }
    };
    options.signal?.addEventListener("abort", onAbort, { once: true });
    // An abort between the initial check and listener registration must still stop this child.
    if (options.signal?.aborted) {
      onAbort();
    }
    const finish = () => {
      options.signal?.removeEventListener("abort", onAbort);
      if (killTimer !== undefined) {
        clearTimeout(killTimer);
      }
    };
    child.once("error", (error) => {
      finish();
      reject(
        new FxError(`grida-fx could not be started (${engine}): ${error.message}`, {
          args,
          cause: error,
        }),
      );
    });
    child.once("exit", (status, signal) => {
      finish();
      // What the engine wrote before it exited is in the pipes: read it, then stop reading.
      const closed = new Promise<void>((done) => child.once("close", () => done()));
      const drained = new Promise<void>((done) => setTimeout(done, DRAIN_MS).unref());
      void Promise.race([closed, drained]).then(() => {
        child.stdout.destroy();
        child.stderr.destroy();
        if (options.signal?.aborted) {
          reject(options.signal.reason);
          return;
        }
        resolvePromise({
          status,
          signal,
          stdout: Buffer.concat(stdout).toString("utf8"),
          stderr: Buffer.concat(stderr).toString("utf8"),
          args,
        });
      });
    });
  });
}

/** The error of a call that did not end as `verb` should. */
export function failure(verb: string, done: Exit): FxError {
  const details = {
    exitCode: done.status,
    signal: done.signal,
    stdout: done.stdout,
    stderr: done.stderr,
    args: done.args,
  };
  if (done.status === USAGE_OR_ERROR) {
    let text = done.stderr.trim() || done.stdout.trim();
    if (text.startsWith(ERROR_PREFIX)) {
      text = text.slice(ERROR_PREFIX.length);
    }
    return new FxError(text || `grida-fx exited with status ${done.status}`, details);
  }
  if (done.status === INTERRUPTED) {
    return new FxError(`grida-fx ${verb} was interrupted`, details);
  }
  const how =
    done.signal !== null ? `was ended by ${done.signal}` : `exited with status ${done.status}`;
  const tail = (done.stderr.trim() || done.stdout.trim()).slice(-2000);
  return new FxError(`grida-fx ${verb} ${how}${tail ? `: ${tail}` : ""}`, details);
}

/** The JSON document a verb prints, read when it exits with one of `statuses`. */
export async function document<T>(
  verb: string,
  args: readonly string[],
  options: CallOptions,
  statuses: readonly number[] = [0, 1],
): Promise<T> {
  const done = await call([verb, ...args], options);
  if (done.status === null || !statuses.includes(done.status)) {
    throw failure(verb, done);
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(done.stdout);
  } catch {
    throw failure(verb, done);
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new FxError(`grida-fx ${verb} printed no JSON object`, {
      exitCode: done.status,
      stdout: done.stdout,
      stderr: done.stderr,
      args: done.args,
    });
  }
  return parsed as T;
}
