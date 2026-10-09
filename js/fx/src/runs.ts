/** A project's runs through the engine's public command line (spec/store.md §8 and §9): listing
 * them, and removing named ones. The engine finds the project from cwd, selects, locks and
 * removes; this module only reads what it prints. */

import { realpathSync } from "node:fs";
import { resolve } from "node:path";
import { type CallOptions, call, type Exit, failure, USAGE_OR_ERROR } from "./engine.js";
import { FxError } from "./errors.js";

/** Where a run sits: `allocated` (`<runs>/<id>/<YYYY-MM-DD>-<n>`), `named`, `explicit`
 * (elsewhere in the runs tree) or `external` (outside it, known from the run index or catalog). */
export type RunPlacement = "allocated" | "named" | "explicit" | "external";

/** A run's state as the listing reads its log. `empty`: no plan.json at a run's position;
 * `removing`: a removal's tombstone; `missing`: an external folder that is gone. Recorded
 * `unfinished` is not evidence that a process is alive. */
export type RunState =
  | "planned"
  | "unfinished"
  | "succeeded"
  | "failed"
  | "incomplete"
  | "cancelled"
  | "empty"
  | "removing"
  | "missing";

/** One run of fx-run-list-v1. Wire fields retain lower_snake_case names. */
export interface RunEntry {
  /** The run folder as the engine printed it: relative to cwd, or absolute. */
  readonly folder: string;
  /** `folder` resolved once against cwd. */
  readonly runDir: string;
  /** The recorded workflow id, or the one its position gives; null when neither is known. */
  readonly workflow: string | null;
  /** The recorded workflow source; null for a folder without a plan. */
  readonly source: string | null;
  /** The run's name: recorded, or given by a named position. */
  readonly name: string | null;
  readonly placement: RunPlacement;
  readonly state: RunState;
  /** The first start's creation time (RFC 3339); null for a folder without a plan. */
  readonly created_at: string | null;
  /** What the run was charged; null before it ended. */
  readonly charged_usd: number | null;
  readonly stand_in: boolean;
  /** Why an incomplete run stopped. */
  readonly stopped?: string;
}

/** What removing one run did, or would do in a preview. */
export interface RunRemovalEntry extends RunEntry {
  /** `would_*`: a preview. `forgotten`: a missing external folder's index and catalog entries
   * removed. `refused`: nothing of this run was removed. `partial`: the run is gone as a run but
   * its tombstone remains; removing it again finishes it. */
  readonly outcome: "would_remove" | "would_forget" | "removed" | "forgotten" | "refused" | "partial";
  /** Present when refused or partial. */
  readonly code?: "active" | "holds_pick" | "changed" | "not_removable" | "io";
  /** Present when refused or partial. */
  readonly message?: string;
  /** Bytes of files whose every link was inside the run folder. */
  readonly freed_bytes: number;
  /** Bytes of files still linked from elsewhere (the store, other runs); not freed. */
  readonly cache_bytes: number;
}

/** Why a selection passed a run over. Explicit runs are never passed over, so a removal by
 * this SDK normally reports none. */
export type RunRemovalSkip = "named" | "explicit" | "external" | "holds_pick" | "young" | "holds_runs";

/** fx-run-removal-v1: what a removal did, or would do when `applied` is false. */
export interface RunRemoval {
  /** False for a preview: nothing was removed. */
  readonly applied: boolean;
  readonly runs: readonly RunRemovalEntry[];
  /** How many runs a selection passed over, by reason. */
  readonly skipped: Readonly<Partial<Record<RunRemovalSkip, number>>>;
  readonly freed_bytes: number;
  readonly cache_bytes: number;
}

/** The fx-run-removal-v1 error document: a RUN the engine would not resolve. Nothing was removed. */
export interface RunRemovalFailure {
  readonly kind: "fx-run-removal-v1";
  readonly applied: false;
  readonly error: {
    readonly code: "invalid_target" | "ambiguous_target" | "not_a_run";
    readonly message: string;
    readonly run: string | null;
  };
}

/** A removal the engine refused before removing anything, with its stable code, the RUN it
 * names and the complete error document. Usage mistakes and malformed output reject with
 * ordinary FxError; a refused or partial run does not reject (read each entry's `outcome`). */
export class RunRemovalError extends FxError {
  readonly code: RunRemovalFailure["error"]["code"];
  readonly run: string | null;
  readonly result: RunRemovalFailure;

  constructor(result: RunRemovalFailure, done: Exit) {
    super(result.error.message, {
      exitCode: done.status,
      signal: done.signal,
      stdout: done.stdout,
      stderr: done.stderr,
      args: done.args,
    });
    this.name = "RunRemovalError";
    this.code = result.error.code;
    this.run = result.error.run;
    this.result = result;
  }
}

/** Options of listRuns. */
export interface ListRunsOptions extends CallOptions {
  /** Only the runs of this workflow id. */
  readonly workflow?: string;
}

/** Options of removeRuns. */
export interface RemoveRunsOptions extends CallOptions {
  /** Report what would be removed and remove nothing (default: false). */
  readonly preview?: boolean;
}

const LIST_KIND = "fx-run-list-v1";
const REMOVAL_KIND = "fx-run-removal-v1";
const PLACEMENTS: readonly string[] = ["allocated", "named", "explicit", "external"];
const STATES: readonly string[] = [
  "planned", "unfinished", "succeeded", "failed", "incomplete", "cancelled", "empty", "removing", "missing",
];
const PREVIEW_OUTCOMES: readonly string[] = ["would_remove", "would_forget", "refused"];
const APPLIED_OUTCOMES: readonly string[] = ["removed", "forgotten", "refused", "partial"];
const FAILED_OUTCOMES: readonly string[] = ["refused", "partial"];
const RUN_CODES: readonly string[] = ["active", "holds_pick", "changed", "not_removable", "io"];
const REMOVAL_SKIPS: readonly string[] = ["named", "explicit", "external", "holds_pick", "young", "holds_runs"];
const ERROR_CODES: readonly string[] = ["invalid_target", "ambiguous_target", "not_a_run"];

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function nullableText(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

function count(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function entry(value: unknown, cwd: string): RunEntry {
  if (!object(value)) throw new Error("run is not an object");
  const { folder, workflow, source, name, placement, state, created_at, charged_usd, stand_in, stopped } = value;
  if (typeof folder !== "string" || folder === "" || folder.includes("\0")) throw new Error("invalid folder");
  if (!nullableText(workflow) || !nullableText(source) || !nullableText(name) || !nullableText(created_at)) {
    throw new Error("invalid run text field");
  }
  if (!PLACEMENTS.includes(placement as string) || !STATES.includes(state as string)) {
    throw new Error("unknown placement or state");
  }
  if (charged_usd !== null && (typeof charged_usd !== "number" || !Number.isFinite(charged_usd))) {
    throw new Error("invalid charged_usd");
  }
  if (typeof stand_in !== "boolean" || (stopped !== undefined && typeof stopped !== "string")) {
    throw new Error("invalid stand_in or stopped");
  }
  return {
    folder,
    runDir: resolve(cwd, folder),
    workflow,
    source,
    name,
    placement: placement as RunPlacement,
    state: state as RunState,
    created_at,
    charged_usd,
    stand_in,
    ...(stopped !== undefined ? { stopped } : {}),
  };
}

function parseList(done: Exit, cwd: string): RunEntry[] {
  if (done.status !== 0) throw new Error("unexpected status");
  const value: unknown = JSON.parse(done.stdout);
  if (!object(value) || value.kind !== LIST_KIND || !Array.isArray(value.runs) || !Array.isArray(value.skipped)) {
    throw new Error("incompatible result");
  }
  // What could not be read is not returned, so its entries are not read: a future code must
  // not break listing.
  return value.runs.map((run) => entry(run, cwd));
}

function removalEntry(value: unknown, cwd: string, applied: boolean): RunRemovalEntry {
  const run = entry(value, cwd);
  const { outcome, code, message, freed_bytes, cache_bytes } = value as Record<string, unknown>;
  if (!(applied ? APPLIED_OUTCOMES : PREVIEW_OUTCOMES).includes(outcome as string)) {
    throw new Error("unknown or mismatched outcome");
  }
  if (code !== undefined && !RUN_CODES.includes(code as string)) throw new Error("unknown code");
  if (message !== undefined && typeof message !== "string") throw new Error("invalid message");
  if (FAILED_OUTCOMES.includes(outcome as string) && (code === undefined || message === undefined)) {
    throw new Error("refusal lacks code or message");
  }
  if (!count(freed_bytes) || !count(cache_bytes)) throw new Error("invalid byte counts");
  return {
    ...run,
    outcome: outcome as RunRemovalEntry["outcome"],
    ...(code !== undefined ? { code: code as NonNullable<RunRemovalEntry["code"]> } : {}),
    ...(message !== undefined ? { message } : {}),
    freed_bytes,
    cache_bytes,
  };
}

/** The error document of exit status 2, or null when stdout holds none. */
function removalFailure(done: Exit): RunRemovalFailure | null {
  let value: unknown;
  try { value = JSON.parse(done.stdout); } catch { return null; }
  if (!object(value) || value.kind !== REMOVAL_KIND || value.applied !== false || !object(value.error)) {
    return null;
  }
  const { code, message, run } = value.error;
  if (!ERROR_CODES.includes(code as string) || typeof message !== "string" || !nullableText(run)) {
    return null;
  }
  return { kind: REMOVAL_KIND, applied: false, error: { code: code as RunRemovalFailure["error"]["code"], message, run } };
}

function parseRemoval(done: Exit, cwd: string, preview: boolean): RunRemoval {
  const value: unknown = JSON.parse(done.stdout);
  if (!object(value) || value.kind !== REMOVAL_KIND || "error" in value || value.applied !== !preview) {
    throw new Error("incompatible result");
  }
  const { runs, skipped, freed_bytes, cache_bytes } = value;
  if (!Array.isArray(runs) || !object(skipped) || !count(freed_bytes) || !count(cache_bytes)) {
    throw new Error("missing or invalid fields");
  }
  for (const [reason, passed] of Object.entries(skipped)) {
    if (!REMOVAL_SKIPS.includes(reason) || !count(passed) || passed < 1) throw new Error("invalid skipped");
  }
  const entries = runs.map((run) => removalEntry(run, cwd, !preview));
  const failed = entries.some((run) => FAILED_OUTCOMES.includes(run.outcome));
  if (done.status !== (failed ? 1 : 0)) throw new Error("mismatched status");
  return {
    applied: !preview,
    runs: entries,
    skipped: { ...skipped } as RunRemoval["skipped"],
    freed_bytes,
    cache_bytes,
  };
}

function callOptions(options: CallOptions, cwd: string): CallOptions {
  return {
    cwd,
    ...(options.env !== undefined ? { env: options.env } : {}),
    ...(options.signal !== undefined ? { signal: options.signal } : {}),
  };
}

/** The runs of the project found from cwd, newest first. Reads only: no lock, no writes.
 * Folders the engine could not read are left out. */
export async function listRuns(options: ListRunsOptions = {}): Promise<RunEntry[]> {
  const args = ["runs", "list"];
  if (options.workflow !== undefined) {
    if (typeof options.workflow !== "string" || options.workflow === "" || options.workflow.includes("\0")) {
      throw new TypeError("workflow is a nonempty workflow id without NUL");
    }
    args.push(`--workflow=${options.workflow}`);
  }
  const cwd = base(options.cwd);
  const done = await call([...args, "--json"], callOptions(options, cwd));
  try {
    return parseList(done, cwd);
  } catch {
    throw failure("runs list", done);
  }
}

/** The working directory as the engine sees it: its real path, since the engine prints folders
 * relative to its canonical working directory (resolved lexically when it cannot be read). */
function base(cwd: string | undefined): string {
  const given = resolve(cwd ?? process.cwd());
  try {
    return realpathSync(given);
  } catch {
    return given;
  }
}

/** Remove exactly the named runs: run folders (relative to cwd) or `WORKFLOW_ID/NAME` of named
 * runs; select them with {@link listRuns} first. Every RUN is resolved before anything is removed,
 * and one that is not a run rejects with {@link RunRemovalError}. A run that is active, or the last
 * holding a pick, is refused without rejecting: read each entry's `outcome`. Aborting a removal
 * may leave a run `partial`; removing it again finishes it. */
export async function removeRuns(runs: readonly string[], options: RemoveRunsOptions = {}): Promise<RunRemoval> {
  if (!Array.isArray(runs) || runs.length === 0
    || !runs.every((run) => typeof run === "string" && run !== "" && !run.includes("\0"))) {
    throw new TypeError("runs is a nonempty array of nonempty run folders or WORKFLOW_ID/NAME, without NUL");
  }
  if (options.preview !== undefined && typeof options.preview !== "boolean") {
    throw new TypeError("preview is true or false");
  }
  const preview = options.preview === true;
  const cwd = base(options.cwd);
  const done = await call(
    ["runs", "remove", ...(preview ? [] : ["--yes"]), "--json", "--", ...runs],
    callOptions(options, cwd),
  );
  if (done.status === USAGE_OR_ERROR) {
    const refused = removalFailure(done);
    if (refused !== null) throw new RunRemovalError(refused, done);
    throw failure("runs remove", done);
  }
  try {
    return parseRemoval(done, cwd, preview);
  } catch {
    if (preview) throw failure("runs remove", done);
    const details = {
      exitCode: done.status, signal: done.signal, stdout: done.stdout, stderr: done.stderr, args: done.args,
    };
    const what = done.status === 0 || done.status === 1
      ? "grida-fx runs remove printed a result that could not be read"
      : failure("runs remove", done).message;
    throw new FxError(`${what}; the removal may already have taken effect`, details);
  }
}
