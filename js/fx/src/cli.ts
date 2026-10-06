/**
 * The `grida-fx` command that `@grida/fx` installs: it finds the engine (`binary.ts`) and runs it
 * with this process's arguments, standard streams and working directory, and ends as the engine
 * ended: with its exit status, or by the same signal.
 *
 * Signals. On Unix the engine runs in a process group (and session) of its own, so a signal meant
 * for the command reaches the engine exactly once, through this process: SIGINT, SIGTERM, SIGHUP
 * and SIGQUIT are passed on. The engine reads a second interruption as "end now", so the same
 * signal arriving again within {@link REPEAT_MS} is passed on once: a launcher such as `npm exec`
 * forwards the terminal's Ctrl-C to a process that already received it. Ctrl-Z (SIGTSTP) stops
 * the engine and then this process; SIGCONT continues both. The engine never reads standard
 * input, so it does not need the terminal's foreground group.
 */

import { type ChildProcess, spawn } from "node:child_process";
import { constants } from "node:os";
import { binary } from "./binary.js";

/** Signals passed on to the engine. */
export const FORWARDED = ["SIGINT", "SIGTERM", "SIGHUP", "SIGQUIT"] as const;
/** The same signal again within this many milliseconds is a repeat of one interruption. */
export const REPEAT_MS = 250;

/** The exit status of the command's own failures (no engine for this machine, …). */
const FAILED = 1;

/** Runs the engine with `argv` (the arguments after the command's name) and ends this process as
 * the engine ended. */
export function main(argv: readonly string[]): void {
  let engine: string;
  try {
    engine = binary();
  } catch (error) {
    process.stderr.write(`grida-fx: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(FAILED);
  }
  const unix = process.platform !== "win32";
  const child = spawn(engine, [...argv], { stdio: "inherit", detached: unix });
  const listeners = forwardSignals(child, unix);
  child.once("error", (error) => {
    listeners.remove();
    process.stderr.write(`grida-fx: could not start the engine (${engine}): ${error.message}\n`);
    process.exit(FAILED);
  });
  child.once("exit", (status, signal) => {
    listeners.remove();
    if (signal !== null) {
      raise(signal);
      return;
    }
    process.exit(status ?? FAILED);
  });
  // Should this process end some other way first, the engine is not left running alone.
  process.once("exit", () => {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill("SIGTERM");
    }
  });
}

/** Passes signals on to `child` (module comment); `remove` stops. */
export function forwardSignals(child: ChildProcess, unix: boolean): { remove(): void } {
  const last = new Map<string, number>();
  const handlers = new Map<NodeJS.Signals, () => void>();
  const pass = (signal: NodeJS.Signals) => {
    const now = Date.now();
    const before = last.get(signal);
    last.set(signal, now);
    if (before !== undefined && now - before < REPEAT_MS) {
      return;
    }
    if (child.exitCode === null && child.signalCode === null) {
      child.kill(signal);
    }
  };
  for (const signal of FORWARDED) {
    if (!unix && signal !== "SIGINT" && signal !== "SIGTERM") {
      continue;
    }
    handlers.set(signal, () => pass(signal));
  }
  if (unix) {
    handlers.set("SIGTSTP", () => {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill("SIGSTOP");
      }
      process.kill(process.pid, "SIGSTOP");
    });
    handlers.set("SIGCONT", () => {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill("SIGCONT");
      }
    });
  }
  for (const [signal, handler] of handlers) {
    process.on(signal, handler);
  }
  return {
    remove() {
      for (const [signal, handler] of handlers) {
        process.removeListener(signal, handler);
      }
    },
  };
}

/** Ends this process by `signal`, as the engine ended; by status 128 + its number when this
 * process outlives it (Node itself handles a few signals). */
function raise(signal: NodeJS.Signals): void {
  const number = constants.signals[signal] ?? 0;
  process.removeAllListeners(signal);
  try {
    process.kill(process.pid, signal);
  } catch {
    // Fall through to the exit status.
  }
  setTimeout(() => process.exit(128 + number), 100);
}
