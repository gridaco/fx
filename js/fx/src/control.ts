/** Exact-target run control through the engine's public command line (spec/control.md).
 * The engine resolves and authenticates the owner and verifies cleanup. */

import { type CallOptions, call, type Exit, failure } from "./engine.js";
import { FxError } from "./errors.js";

/** The engine's fx-run-control-v1 response. Wire fields retain lower_snake_case names. */
export interface RunControlResult {
  readonly kind: "fx-run-control-v1";
  readonly operation: "inspect" | "cancel";
  readonly invocation_id: string | null;
  readonly outcome:
    | "inspected"
    | "accepted"
    | "already_requested"
    | "already_terminal"
    | "finishing"
    | "completed"
    | "error";
  readonly request_status: "accepted" | "not_accepted" | "unknown";
  readonly recorded_state: "planned" | "unfinished" | "succeeded" | "failed" | "cancelled" | "unknown";
  readonly cleanup: "pending" | "complete" | "unknown";
  readonly external_completion: "not_verified";
  readonly code?: string;
  readonly message?: string;
  readonly availability?: "available" | "unavailable" | "unsupported" | "unknown";
  readonly can_cancel?: boolean;
}

/** A structured control failure, retaining its stable code and complete result.
 * Unsupported binaries and malformed responses reject with ordinary FxError. */
export class RunControlError extends FxError {
  readonly result: RunControlResult;
  readonly code: string;

  constructor(result: RunControlResult, done: Exit) {
    super(result.message ?? `run control failed: ${result.code}`, {
      exitCode: done.status,
      signal: done.signal,
      stdout: done.stdout,
      stderr: done.stderr,
      args: done.args,
    });
    this.name = "RunControlError";
    this.result = result;
    this.code = result.code ?? "";
  }
}

/** Options of cancel. Relative targets start at cwd, as on the command line. */
export interface CancelOptions extends CallOptions {
  /** Expected-current invocation guard. A successor is refused, never automatically selected. */
  readonly invocation?: string;
  /** Verify local completion, with a default timeout of thirty seconds. */
  readonly wait?: boolean;
  /** Positive integer followed by s, m or h (e.g. "5s"); requires wait: true. This bounds
   * waiting only and never terminates the run or retracts an accepted cancellation. */
  readonly timeout?: string;
}

const MAX_RESULT_BYTES = 16 * 1024;
const REQUIRED = [
  "kind", "operation", "invocation_id", "outcome", "request_status", "recorded_state",
  "cleanup", "external_completion",
] as const;
const OPTIONAL = ["code", "message", "availability", "can_cancel"];
const ERROR_STATUS: Readonly<Record<string, number>> = {
  unavailable: 1,
  wait_timeout: 1,
  owner_lost: 1,
  acknowledgment_unknown: 1,
  completion_unverified: 1,
  invalid_target: 2,
  ambiguous_target: 2,
  invocation_mismatch: 2,
  unsupported_version: 2,
  unauthorized: 2,
  record_error: 2,
};
const ENUMS: Readonly<Record<string, readonly string[]>> = {
  outcome: ["inspected", "accepted", "already_requested", "already_terminal", "finishing", "completed", "error"],
  request_status: ["accepted", "not_accepted", "unknown"],
  recorded_state: ["planned", "unfinished", "succeeded", "failed", "cancelled", "unknown"],
  cleanup: ["pending", "complete", "unknown"],
  external_completion: ["not_verified"],
};

function target(run: string): string {
  if (typeof run !== "string" || run === "" || run.startsWith("-") || run.includes("\0")) {
    throw new TypeError("run is a nonempty explicit run folder or exact WORKFLOW_ID/NAME, without a leading '-' or NUL");
  }
  return run;
}

function cancelOptions(options: CancelOptions): string[] {
  if (options.wait !== undefined && typeof options.wait !== "boolean") {
    throw new TypeError("wait is true or false");
  }
  const args = ["--source=sdk"];
  if (options.invocation !== undefined) {
    if (typeof options.invocation !== "string" || options.invocation === "" || options.invocation.includes("\0")) {
      throw new TypeError("invocation is a nonempty invocation ID");
    }
    args.push(`--invocation=${options.invocation}`);
  }
  if (options.wait === true) {
    args.push("--wait");
  }
  if (options.timeout !== undefined) {
    if (options.wait !== true) {
      throw new TypeError("timeout requires wait: true");
    }
    if (typeof options.timeout !== "string" || !/^[0-9]+[smh]$/.test(options.timeout) || /^0+[smh]$/.test(options.timeout)) {
      throw new TypeError("timeout is a positive integer followed by s, m or h");
    }
    args.push(`--timeout=${options.timeout}`);
  }
  return args;
}

function parseResult(done: Exit, operation: "inspect" | "cancel", wait: boolean): RunControlResult {
  if (Buffer.byteLength(done.stdout, "utf8") > MAX_RESULT_BYTES) {
    throw new Error("oversized result");
  }
  const value: unknown = JSON.parse(done.stdout);
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("not an object");
  }
  const data = value as Record<string, unknown>;
  if (!REQUIRED.every((name) => name in data)) {
    throw new Error("missing fields");
  }
  for (const [name, field] of Object.entries(data)) {
    if (![...REQUIRED, ...OPTIONAL].includes(name)) continue;
    if (name === "can_cancel") {
      if (typeof field !== "boolean") throw new Error("can_cancel is not boolean");
    } else if (name !== "invocation_id" || field !== null) {
      if (typeof field !== "string" || field === "" || field.length > (name === "invocation_id" ? 256 : 4096)) {
        throw new Error(`invalid ${name}`);
      }
    }
  }
  if (data.kind !== "fx-run-control-v1" || data.operation !== operation) {
    throw new Error("incompatible result");
  }
  if (Object.entries(ENUMS).some(([name, values]) => !values.includes(data[name] as string))) {
    throw new Error("unknown result enum");
  }
  if (data.outcome === "completed" && (data.invocation_id === null || data.cleanup !== "complete"
    || !["succeeded", "failed", "cancelled"].includes(data.recorded_state as string))) {
    throw new Error("completion lacks invocation, cleanup or terminal evidence");
  }
  if (operation === "inspect") {
    if (!["available", "unavailable", "unsupported", "unknown"].includes(data.availability as string) || typeof data.can_cancel !== "boolean") {
      throw new Error("missing or unknown inspection fields");
    }
  } else if ("availability" in data || "can_cancel" in data) {
    throw new Error("inspection fields on cancellation");
  }
  if (data.outcome === "error") {
    if (ERROR_STATUS[data.code as string] !== done.status || !("message" in data)) {
      throw new Error("unknown code or mismatched error status");
    }
  } else {
    const allowed = operation === "inspect" ? ["inspected"] : wait ? ["completed"] : ["accepted", "already_requested", "already_terminal", "finishing"];
    if (done.status !== 0 || !allowed.includes(data.outcome as string) || "code" in data || "message" in data) {
      throw new Error("mismatched success outcome or status");
    }
  }
  return data as unknown as RunControlResult;
}

async function control(
  operation: "inspect" | "cancel", run: string, options: CallOptions, args: readonly string[], wait = false,
): Promise<RunControlResult> {
  const verb = operation === "inspect" ? "inspect" : "cancel";
  const done = await call([verb, target(run), ...args, "--json"], options);
  let result: RunControlResult;
  try {
    result = parseResult(done, operation, wait);
  } catch {
    throw failure(verb, done);
  }
  if (result.outcome === "error") {
    throw new RunControlError(result, done);
  }
  return result;
}

/** Inspect one exact target's invocation and authenticated control availability. A bare
 * workflow ID is refused by the engine; no latest-run selection is implied. */
export async function inspectControl(run: string, options: CallOptions = {}): Promise<RunControlResult> {
  return control("inspect", run, options, ["--control"]);
}

/** Request cancellation of one invocation. Aborting this control client only ends its wait;
 * an accepted cancellation remains effective in the separately owned runner. */
export async function cancel(run: string, options: CancelOptions = {}): Promise<RunControlResult> {
  return control("cancel", run, options, cancelOptions(options), options.wait === true);
}
