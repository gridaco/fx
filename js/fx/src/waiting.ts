/** External answers to a concrete waiting step. */
import { call, failure, type CallOptions, type Exit } from "./engine.js";
import { FxError } from "./errors.js";
import type { JsonSchemaObject } from "./node.js";
import { checkValue, isPlainObject } from "./protocol.js";
import type { JsonInput, JsonValue } from "./workflow.js";

export type AnswerState =
  | { readonly status: "pending" }
  | { readonly status: "answered"; readonly answer: JsonValue; readonly channel: "cli" | "sdk" | "viewer"; readonly at: string }
  | { readonly status: "expired"; readonly at: string };

export interface WaitingRequest {
  readonly id: string;
  readonly step: string;
  readonly message?: string;
  readonly schema: JsonSchemaObject | boolean;
  readonly choices?: Readonly<Record<string, JsonValue>>;
  readonly show: JsonValue;
  readonly choice_field?: string;
  readonly timeout_seconds?: number;
  readonly on_timeout: "fail" | "continue";
  readonly opened_at: string;
  readonly deadline?: string;
  readonly state: AnswerState;
}

export class AnswerError extends FxError {
  constructor(
    readonly code: string,
    message: string,
    readonly request: WaitingRequest | undefined,
    status: number,
  ) {
    super(message, { exitCode: status });
    this.name = "AnswerError";
  }
}

function readReply(verb: string, done: Exit): Record<string, unknown> {
  try {
    const body: unknown = JSON.parse(done.stdout);
    if (isPlainObject(body)) return body;
  } catch {
    // A malformed engine reply is reported with its exit status below.
  }
  throw failure(verb, done);
}

function isRequest(value: unknown): value is WaitingRequest {
  if (!isPlainObject(value) || typeof value.id !== "string" || !/^[0-9a-f]{64}$/.test(value.id)
    || typeof value.step !== "string" || typeof value.opened_at !== "string"
    || !(typeof value.schema === "boolean" || isPlainObject(value.schema)) || !("show" in value)
    || (value.on_timeout !== "fail" && value.on_timeout !== "continue")
    || ["message", "choice_field", "deadline"].some(key => key in value && typeof value[key] !== "string")
    || ("choices" in value && !isPlainObject(value.choices))
    || ("timeout_seconds" in value && !(typeof value.timeout_seconds === "number"
      && Number.isSafeInteger(value.timeout_seconds) && value.timeout_seconds > 0))) return false;
  const state = value.state;
  if (!isPlainObject(state)) return false;
  if (state.status === "pending") return true;
  if (typeof state.at !== "string") return false;
  return state.status === "expired" || (state.status === "answered" && "answer" in state
    && (state.channel === "cli" || state.channel === "sdk" || state.channel === "viewer"));
}

async function requestInspection(run: string, options: CallOptions, viewer?: "--register" | "--viewer-url") {
  const args = ["inspect", "--requests", "--json"];
  if (viewer) args.push(viewer);
  const done = await call([...args, "--", run], options);
  if (done.status !== 0) throw failure("inspect", done);
  const result = readReply("inspect", done);
  if (!Array.isArray(result.requests) || !result.requests.every(isRequest)) throw failure("inspect", done);
  const url = viewer ? result.viewer_url : null;
  if (url !== null && typeof url !== "string") throw failure("inspect", done);
  return { requests: result.requests, viewer_url: url };
}

/** Inspect requests and discover the local viewer URL. Registration is explicit. */
export async function inspectRequests(
  run: string, options: CallOptions & { register?: boolean } = {},
): Promise<{
  requests: WaitingRequest[];
  viewer_url: string | null;
}> {
  return requestInspection(run, options, options.register ? "--register" : "--viewer-url");
}

/** List pending and resolved requests in the exact run folder or workflow/name. */
export async function listRequests(run: string, options: CallOptions = {}): Promise<WaitingRequest[]> {
  return (await requestInspection(run, options)).requests;
}

/** Record a schema-validated answer. Conflicts and expirations retain the winning request. */
export async function respond(
  run: string, requestId: string, answer: JsonInput, options: CallOptions = {},
): Promise<WaitingRequest> {
  checkValue(answer);
  const done = await call(
    ["respond", "--source=sdk", "--json=-", "--", run, requestId], options, JSON.stringify(answer),
  );
  const receipt = readReply("respond", done);
  const request = receipt.request;
  if (request !== undefined && (!isRequest(request) || request.id !== requestId)) throw failure("respond", done);
  if (done.status === 0 && request?.state.status === "answered") return request;
  if ((done.status === 1 || done.status === 2) && typeof receipt.code === "string" && typeof receipt.message === "string") {
    if ((receipt.code === "answer_conflict" && request?.state.status !== "answered")
      || (receipt.code === "expired" && request?.state.status !== "expired")) throw failure("respond", done);
    throw new AnswerError(receipt.code, receipt.message, request, done.status);
  }
  throw failure("respond", done);
}
