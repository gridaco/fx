/** Saved evidence and independent observation; neither owns execution or a viewer. */

import { resolve } from "node:path";
import type { InspectOptions } from "./api.js";
import type { Graph, Inspection, RunEvent } from "./documents.js";
import { call, type CallOptions, type Exit, failure } from "./engine.js";
import { FxError } from "./errors.js";

/** The complete recorded event envelope. Unknown events and fields remain readable. */
export interface ObservedRunEvent extends RunEvent {
  kind: "fx-run-events-v1";
  invocation_id: string;
  plan: string;
  offset_ms: number;
}

export interface RunSnapshot {
  kind: "fx-run-snapshot-v1";
  cursor: string;
  plan: Graph;
  events: ObservedRunEvent[];
  [field: string]: unknown;
}

export interface RunEventBatch {
  kind: "fx-run-event-batch-v1";
  cursor: string;
  events: ObservedRunEvent[];
  has_more: boolean;
  [field: string]: unknown;
}

export interface RunObservationFailure {
  kind: "fx-run-observation-error-v1";
  code: string;
  message: string;
  [field: string]: unknown;
}

/** A reader failure with the engine's stable code and complete public error document. */
export class RunObservationError extends FxError {
  readonly code: string;
  readonly document: RunObservationFailure;

  constructor(document: RunObservationFailure, done: Exit) {
    super(document.message, {
      exitCode: done.status, signal: done.signal, stdout: done.stdout,
      stderr: done.stderr, args: done.args,
    });
    this.name = "RunObservationError";
    this.code = document.code;
    this.document = document;
  }
}

export interface RunReadOptions {
  /** Stops this reader only; it never cancels the separately owned workflow execution. */
  readonly signal?: AbortSignal;
}

export interface RunEventsOptions extends RunReadOptions {
  /** Opaque acknowledged cursor; omit to read from the beginning. */
  readonly after?: string;
  /** Maximum events per batch, 1–1024 (default: 256). */
  readonly limit?: number;
}

export interface RunFollowOptions extends RunEventsOptions {
  /** Poll only after draining readable batches (default: 1000 milliseconds). */
  readonly pollIntervalMs?: number;
}

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function text(value: unknown): value is string {
  return typeof value === "string" && value !== "";
}

function cursor(value: unknown): value is string {
  return text(value) && !value.includes("\0");
}

function event(value: unknown): value is ObservedRunEvent {
  return object(value) && value.kind === "fx-run-events-v1" && text(value.event)
    && text(value.invocation_id) && typeof value.plan === "string"
    && /^[a-f0-9]{64}$/.test(value.plan) && Number.isSafeInteger(value.offset_ms)
    && (value.offset_ms as number) >= 0;
}

function response(done: Exit, snapshot: boolean, options: RunEventsOptions = {}): RunSnapshot | RunEventBatch {
  let value: unknown;
  try { value = JSON.parse(done.stdout); } catch { throw failure("observe", done); }
  if (done.status === 2 && object(value) && value.kind === "fx-run-observation-error-v1"
    && text(value.code) && text(value.message)) {
    throw new RunObservationError(value as unknown as RunObservationFailure, done);
  }
  if (done.status !== 0 || !object(value) || !cursor(value.cursor)
    || !Array.isArray(value.events) || !value.events.every(event)) {
    throw failure("observe", done);
  }
  if (snapshot) {
    if (value.kind !== "fx-run-snapshot-v1" || !object(value.plan)
      || value.plan.kind !== "fx-graph-v1") throw failure("observe", done);
    return value as unknown as RunSnapshot;
  }
  if (value.kind !== "fx-run-event-batch-v1" || typeof value.has_more !== "boolean"
    || value.events.length > (options.limit ?? 256)
    || (value.events.length === 0 && (value.has_more || (options.after !== undefined && value.cursor !== options.after)))
    || (value.events.length > 0 && value.cursor === options.after)) {
    throw failure("observe", done);
  }
  return value as unknown as RunEventBatch;
}

function eventArgs(options: RunEventsOptions): string[] {
  const args: string[] = [];
  if (options.after !== undefined) {
    if (!text(options.after) || options.after.includes("\0")) {
      throw new TypeError("after is a nonempty opaque cursor without NUL");
    }
    args.push(`--after=${options.after}`);
  }
  if (options.limit !== undefined) {
    if (!Number.isInteger(options.limit) || options.limit < 1 || options.limit > 1024) {
      throw new TypeError("limit is an integer from 1 to 1024");
    }
    args.push(`--limit=${options.limit}`);
  }
  return args;
}

function sleep(milliseconds: number, signal?: AbortSignal): Promise<void> {
  signal?.throwIfAborted();
  return new Promise((done, reject) => {
    const finish = () => { signal?.removeEventListener("abort", aborted); done(); };
    const timer = setTimeout(finish, milliseconds);
    const aborted = () => {
      clearTimeout(timer);
      signal?.removeEventListener("abort", aborted);
      reject(signal?.reason);
    };
    signal?.addEventListener("abort", aborted, { once: true });
  });
}

/** One resolved run folder and its sampled inspection. Reads stay pinned to this folder.
 * Inspection and later snapshots are independent samples; only each snapshot and its cursor
 * share one consistent prefix. No exit code or process-liveness claim is synthesized. */
export class RunRecord {
  readonly runDir: string;
  readonly inspection: Inspection;
  private readonly context: CallOptions;

  constructor(runDir: string, inspection: Inspection, context: CallOptions = {}) {
    this.runDir = resolve(context.cwd ?? process.cwd(), runDir);
    this.inspection = inspection;
    this.context = {
      cwd: resolve(context.cwd ?? process.cwd()),
      ...(context.env !== undefined ? { env: { ...context.env } } : {}),
    };
  }

  /** Capture the saved plan, complete readable event prefix and matching attachment cursor. */
  async snapshot(options: RunReadOptions = {}): Promise<RunSnapshot> {
    const done = await call(["observe", this.runDir, "--snapshot"], {
      ...this.context, ...(options.signal !== undefined ? { signal: options.signal } : {}),
    });
    return response(done, true) as RunSnapshot;
  }

  /** Read bounded events after a cursor, or from the beginning when after is omitted. */
  async events(options: RunEventsOptions = {}): Promise<RunEventBatch> {
    const args = eventArgs(options);
    const done = await call(["observe", this.runDir, ...args], {
      ...this.context, ...(options.signal !== undefined ? { signal: options.signal } : {}),
    });
    return response(done, false, options) as RunEventBatch;
  }

  /** Follow nonempty batches continuously, including later resumes, until break or abort.
   * Terminal events end an invocation, not this reader, and do not certify owner cleanup.
   * Start after snapshot.cursor to attach without missing events. Reader errors surface;
   * no missing-run waits, automatic reattachment or execution retries are performed. */
  async *follow(options: RunFollowOptions = {}): AsyncGenerator<RunEventBatch> {
    eventArgs(options);
    const interval = options.pollIntervalMs ?? 1000;
    if (!Number.isFinite(interval) || interval <= 0 || interval > 2_147_483_647) {
      throw new TypeError("pollIntervalMs is positive, finite and at most 2147483647");
    }
    let after = options.after;
    for (;;) {
      options.signal?.throwIfAborted();
      const batch = await this.events({
        ...(after !== undefined ? { after } : {}),
        ...(options.limit !== undefined ? { limit: options.limit } : {}),
        ...(options.signal !== undefined ? { signal: options.signal } : {}),
      });
      // A slow consumer or one mutating its received document cannot change our position.
      after = batch.cursor;
      const more = batch.has_more;
      if (batch.events.length > 0) yield batch;
      if (!more) await sleep(interval, options.signal);
    }
  }
}

/** Resolve a folder or recorded selector once, without planning, executing or starting a service.
 * Later reads use the pinned absolute folder. The loading signal is not retained by the record. */
export async function loadRun(target: string, options: InspectOptions = {}): Promise<RunRecord> {
  if (!text(target) || target.startsWith("-") || target.includes("\0")) {
    throw new TypeError("target is a nonempty saved-run selector without a leading '-' or NUL");
  }
  if (options.verify !== undefined && typeof options.verify !== "boolean") {
    throw new TypeError("verify is true or false");
  }
  const cwd = resolve(options.cwd ?? process.cwd());
  const done = await call(["inspect", target, "--json", ...(options.verify === true ? ["--verify"] : [])], {
    cwd,
    ...(options.env !== undefined ? { env: options.env } : {}),
    ...(options.signal !== undefined ? { signal: options.signal } : {}),
  });
  if (done.status !== 0 && !(options.verify === true && done.status === 1)) {
    throw failure("inspect", done);
  }
  let inspection: unknown;
  try { inspection = JSON.parse(done.stdout); } catch { throw failure("inspect", done); }
  if (!object(inspection) || !object(inspection.run)) throw failure("inspect", done);
  if (done.status === 1) {
    const verification = inspection.verification;
    if (!object(verification) || verification.verified !== false
      || !Array.isArray(verification.problems) || verification.problems.length === 0
      || !verification.problems.every(text)) throw failure("inspect", done);
  }
  const folder = inspection.run.folder;
  if (!text(folder) || folder.includes("\0")) {
    throw failure("inspect", done);
  }
  return new RunRecord(resolve(cwd, folder), inspection as unknown as Inspection, {
    cwd, ...(options.env !== undefined ? { env: options.env } : {}),
  });
}
