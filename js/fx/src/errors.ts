/** Errors of `@grida/fx`. */

/** What the engine left behind when it failed. */
export interface FxErrorDetails {
  /** The engine's exit status; `null` when it never started or ended by a signal. */
  readonly exitCode?: number | null;
  /** The signal that ended the engine, if one did. */
  readonly signal?: string | null;
  /** What the engine wrote on stderr. */
  readonly stderr?: string;
  /** What the engine wrote on stdout. */
  readonly stdout?: string;
  /** The arguments the engine was given (after the binary). */
  readonly args?: readonly string[];
  readonly cause?: unknown;
}

/**
 * The engine stopped with an error (exit status 2: unreadable input or a usage mistake), would
 * not run (`refused: …`), was interrupted (130), or could not be found or started. `message` is
 * the engine's own message, without its `grida-fx: ` prefix.
 */
export class FxError extends Error {
  /** The engine's exit status; `null` when it never started or ended by a signal. */
  readonly exitCode: number | null;
  /** The signal that ended the engine, if one did. */
  readonly signal: string | null;
  /** What the engine wrote on stderr. */
  readonly stderr: string;
  /** What the engine wrote on stdout. */
  readonly stdout: string;
  /** The arguments the engine was given (after the binary). */
  readonly args: readonly string[];

  constructor(message: string, details: FxErrorDetails = {}) {
    super(message, details.cause === undefined ? undefined : { cause: details.cause });
    this.name = "FxError";
    this.exitCode = details.exitCode ?? null;
    this.signal = details.signal ?? null;
    this.stderr = details.stderr ?? "";
    this.stdout = details.stdout ?? "";
    this.args = details.args ?? [];
  }
}
