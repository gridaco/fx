/**
 * The command launcher: it finds the engine (`binary.ts`) and runs it with the given arguments,
 * this process's standard streams and working directory, and ends as the engine ended: with its
 * exit status, or by the same signal. `@grida/fx`'s own `grida-fx` command is {@link main}; a
 * host command that carries FX as a verb of its own (the npm `grida` command's `grida fx`) calls
 * {@link launch} through the `@grida/fx/cli` export (spec/distribution.md).
 *
 * The invoker. {@link launch} tells the engine which command started it (`GRIDA_FX_INVOKER`), so
 * usage, the error prefix and the hints the engine prints name the command as its user types it:
 * `grida-fx` or `grida fx`. This launcher's own messages use the same name.
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

/**
 * The version of the contract between `@grida/fx/cli` and a host command: {@link launch}'s
 * signature and behaviour, the invokers it accepts, and what the engine does with each. A host
 * pins `@grida/fx` exactly and refuses a contract it was not written for.
 */
export const HOST_CONTRACT = 1;

/** The environment variable that tells the engine which command started it. */
export const INVOKER_VARIABLE = "GRIDA_FX_INVOKER";

/** A command that launches the engine from JavaScript: `@grida/fx`'s `grida-fx`, or `grida`'s
 * `grida fx`. (`python -m grida.fx` is the Python distribution's.) */
export type Invoker = "grida-fx" | "grida";

/** The command each invoker's user types, without the verb. */
export const COMMANDS: Readonly<Record<Invoker, string>> = {
  "grida-fx": "grida-fx",
  grida: "grida fx",
};

/** Options of {@link launch}. */
export interface LaunchOptions {
  /** The command that started the engine (default: `grida-fx`). */
  readonly invoker?: Invoker;
}

/** Signals passed on to the engine. */
export const FORWARDED = ["SIGINT", "SIGTERM", "SIGHUP", "SIGQUIT"] as const;
/** The same signal again within this many milliseconds is a repeat of one interruption. */
export const REPEAT_MS = 250;

/** The exit status of the command's own failures (no engine for this machine, …). */
const FAILED = 1;

/** `@grida/fx`'s `grida-fx` command: {@link launch} as the `grida-fx` invoker. */
export function main(argv: readonly string[]): void {
  launch(argv, { invoker: "grida-fx" });
}

/** Runs the engine with `argv` (the arguments after the command's name, and after `fx` for
 * `grida fx`) and ends this process as the engine ended. Throws a `TypeError`, before anything
 * runs, for an invoker this contract does not know. */
export function launch(argv: readonly string[], options: LaunchOptions = {}): void {
  const invoker = options.invoker ?? "grida-fx";
  if (!Object.hasOwn(COMMANDS, invoker)) {
    throw new TypeError(
      `unknown invoker ${JSON.stringify(invoker)}: @grida/fx/cli contract ${HOST_CONTRACT} ` +
        `knows ${Object.keys(COMMANDS).join(", ")}`,
    );
  }
  const command = COMMANDS[invoker];
  const env = { ...process.env, [INVOKER_VARIABLE]: invoker };
  let engine: string;
  try {
    engine = binary({ env });
  } catch (error) {
    process.stderr.write(`${command}: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(FAILED);
  }
  const unix = process.platform !== "win32";
  const child = spawn(engine, [...argv], { stdio: "inherit", detached: unix, env });
  const listeners = forwardSignals(child, unix);
  child.once("error", (error) => {
    listeners.remove();
    process.stderr.write(`${command}: could not start the engine (${engine}): ${error.message}\n`);
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
