/**
 * Project adapters (`spec/adapters.md` §9; `spec/protocol.md` §5.8, §6.8 and §8.2): a declared
 * provider's paid calls, served by the project's own TypeScript code in a node host.
 *
 * - **Declaring.** `export const acme = adapter({ requests, jobs, send, submit, collect, version })`
 *   makes an {@link Adapter}, which a host loads by its module and export (`adapter.load`,
 *   `host.ts`). Each capability of `requests` is served by `send`, and each of `jobs` by `submit`
 *   and `collect`, in that order; `version` is an integer of at least 0, or `null`. The types
 *   refuse a declaration without a capability, `requests` without `send`, `jobs` without `submit`
 *   and `collect`, a function that returns an outcome its method does not take, and a field
 *   `adapter()` does not know; each function's call carries the capabilities it serves. The
 *   declaration is checked again when it is made, with the Python SDK's sentences, as a
 *   `TypeError`: `an adapter declaration is an object, not …`; `an adapter declaration has no field
 *   'versoin'`; `adapter(): requests is a list of capability names` (or `jobs`); `adapter():
 *   "Image" is not a capability name`; `adapter(): image.generate is named twice`; `adapter()
 *   declares no capability: give it requests or jobs`; `adapter(): send is a function` (or
 *   `submit`, `collect`); `adapter(): requests need send`; `adapter(): jobs need submit and
 *   collect`; `adapter(): version is an integer of at least 0, or null`. A member whose value is
 *   `undefined` is left out.
 * - **The call.** The params of `adapter.send`, `adapter.submit` and `adapter.collect` become an
 *   {@link AdapterCall}: `capability`, `route` (`id`, `model`, `provider`, `contract`), `baseUrl`,
 *   `key`, `attempt`, `handle` (`undefined` outside `collect`), `takes` (the `take` list) and
 *   `take` (its last number), `files` (an {@link AdapterFile} by digest), `request` (a copy whose
 *   file values, `{"file": <digest>}` at any depth, are those files) and `signal`. Params of
 *   another shape are a `TypeError` with the Python SDK's sentence, which the host answers
 *   `-32602`.
 * - **fetch.** `await call.fetch(url, options)` asks the engine for one exchange (§6.8), with the
 *   call's `call_id` and no `run_id`. Header names are lowercased. Refused before anything is
 *   sent, with a `TypeError`: an option `fetch` does not know (`fetch has no option 'verb'`), a
 *   method other than GET and POST (`fetch takes GET or POST, not PUT`), a body on a GET (`a GET
 *   has no body`), `json` and `body` both (`fetch takes json or body, not both`), a `body` that
 *   is not bytes, a `body` without its `contentType` (`a body needs its contentType`) or the
 *   reverse (`contentType goes with a body`), another lane (`fetch's lane is provider, upload or
 *   download, not 'cdn'`), a header name that is not a token (`the header name "x y" is not a
 *   token`), and the engine's ranges (`timeoutS is above 0 and at most 300`, `maxResponseBytes is
 *   from 1 to 67108864`). The answer is a {@link FetchResponse} (`status`, `headers` as `[name,
 *   value]` pairs in order, `body` bytes, `json()`, `text()`); a failed exchange rejects with a
 *   {@link FetchError} (`phase`: `refused`, `not_sent` or `after_send`; `reason`), and an error
 *   answer with the `EngineError` of its code (`Cancelled` for a call the engine dropped).
 * - **Outcomes** are plain objects named by `outcome` ({@link SendOutcome}, {@link SubmitOutcome},
 *   {@link CollectOutcome}), held to the protocol when they are sent ({@link encodeOutcome}): a
 *   `reason` is a string with a character other than white space, `costUsd` and `retryAfterS`
 *   finite numbers of at least 0, `retryable` a boolean, `usage` an object of `inputTokens`,
 *   `cachedInputTokens` and `outputTokens`, each an integer of at least 0; an answer's files are
 *   a stand-in's, with an `AdapterFile` where a stand-in's take an `InputFile` (bytes leave the
 *   kind out; an `AdapterFile` and `{bytes, kind}` give theirs, a `file:` URL the kind of its
 *   suffix), named `[a-z][a-z0-9_]*`, and its `data`, and a handle, values the engine can read. A
 *   member an outcome does not have is refused (`Answered holds outcome, files, data, costUsd and
 *   usage, not 'cost_usd'`), and one whose value is `undefined` is left out.
 * - **Serving.** {@link AdapterServer} answers calls concurrently, in any order, on the host's one
 *   event loop: a function that never yields holds the host. `$/cancel` answers the call
 *   `cancelled` at once and aborts its `signal`; what the function returns or throws after that
 *   is discarded. Anything the function throws, a value that is not an outcome its method takes
 *   (`send returns Answered, NotReceived, Refused or Failed, not undefined`) and an outcome the
 *   SDK cannot send are faults of the adapter, answered `internal` with `<name>: <message>`, which
 *   ends the call as the loss of its host would; unlike a stand-in's, a fault ends only its own
 *   call.
 */

import { Cancelled, EngineError } from "./engine-errors.js";
import { AdapterFile } from "./files.js";
import {
  brandOf,
  CANCELLED,
  checkValue,
  type ErrorBody,
  encodableText,
  type Id,
  INTERNAL,
  isPlainObject,
  pyRepr,
  safeString,
  typeName,
} from "./protocol.js";
import { ADAPTER_FILES, encodeData, encodeFiles, errorText, withFiles } from "./stand-in.js";
import type { JsonInput, JsonValue } from "./workflow.js";

// ------------------------------------------------------------------------------------------------
// Outcomes

/** What a call consumed besides dollars, as the provider reported it. */
export interface AdapterUsage {
  readonly inputTokens?: number | undefined;
  readonly cachedInputTokens?: number | undefined;
  readonly outputTokens?: number | undefined;
}

/** A file of an answer: bytes (the kind the capability names), an {@link AdapterFile} (its kind),
 * `{bytes, kind}`, or a `file:` URL (the kind of its suffix). */
export type AdapterAnswerFile =
  | Uint8Array
  | AdapterFile
  | { readonly bytes: Uint8Array; readonly kind?: string | undefined }
  | URL;

/** `answered`: the provider's answer, its `files` by name (none when left out) and its `data`
 * (`null` when left out); `costUsd` the cost it reported, and `usage` its token counts. */
export interface Answered {
  readonly outcome: "answered";
  readonly files?: { readonly [name: string]: AdapterAnswerFile | undefined } | undefined;
  readonly data?: JsonInput | undefined;
  readonly costUsd?: number | undefined;
  readonly usage?: AdapterUsage | undefined;
}

/** `not_received`: the provider provably did not receive the request; the engine may send it
 * again, after `retryAfterS` seconds when the provider asked for a wait. */
export interface NotReceived {
  readonly outcome: "not_received";
  readonly reason: string;
  readonly retryAfterS?: number | undefined;
}

/** `refused`: refused before anything was sent, at $0. */
export interface Refused {
  readonly outcome: "refused";
  readonly reason: string;
}

/** `failed`: the provider may have received the request, which failed; billed at `costUsd`, else
 * the whole hold, and made again only when `retryable`. */
export interface Failed {
  readonly outcome: "failed";
  readonly reason: string;
  readonly retryable: boolean;
  readonly costUsd?: number | undefined;
}

/** `accepted`: the provider took the job; the `handle` holds only what collecting needs, never a
 * host, a signed URL or a key. */
export interface Accepted {
  readonly outcome: "accepted";
  readonly handle: { readonly [key: string]: JsonInput | undefined };
}

/** `uncertain`: nobody can say whether the provider took the job. */
export interface Uncertain {
  readonly outcome: "uncertain";
  readonly reason: string;
}

/** `ended`: the job ended without a usable result; settled at `costUsd`, else the whole hold. */
export interface Ended {
  readonly outcome: "ended";
  readonly reason: string;
  readonly costUsd?: number | undefined;
}

/** `unreachable`: it is not clear that the job is over; a later run collects it again. */
export interface Unreachable {
  readonly outcome: "unreachable";
  readonly reason: string;
}

/** What `send` returns. */
export type SendOutcome = Answered | NotReceived | Refused | Failed;
/** What `submit` returns. */
export type SubmitOutcome = Accepted | Uncertain | NotReceived | Refused | Failed;
/** What `collect` returns. */
export type CollectOutcome = Answered | Ended | Unreachable;

/** Answers of the usual shapes. */
export const Answered = {
  /** A `structured.generate` answer: `data` of `{json: value}`. */
  json(
    value: JsonInput,
    options: { readonly costUsd?: number | undefined; readonly usage?: AdapterUsage | undefined } = {},
  ): Answered {
    return { outcome: "answered", data: { json: value }, costUsd: options.costUsd, usage: options.usage };
  },
};

// ------------------------------------------------------------------------------------------------
// The call

/** A job's handle, as `submit` accepted it. */
export type JobHandle = { readonly [key: string]: JsonValue };

/** A value of a call's request: JSON, with files as {@link AdapterFile} objects. */
export type AdapterValue =
  | null
  | boolean
  | number
  | string
  | AdapterFile
  | readonly AdapterValue[]
  | { readonly [key: string]: AdapterValue };

/** `call.route`: the route's `id` (`model@provider`), `model`, `provider`, and its `contract` as
 * its table's reading completed it. */
export interface AdapterRoute {
  readonly id: string;
  readonly model: string;
  readonly provider: string;
  readonly contract: { readonly [member: string]: JsonValue };
}

/** The methods an adapter host answers for a call. */
export type AdapterMethod = "adapter.send" | "adapter.submit" | "adapter.collect";

/** How a call reaches the engine for `fetch`: the params, sent; the result, awaited. */
export type EngineFetch = (params: Record<string, unknown>) => Promise<unknown>;

/** How an {@link AdapterCall} is made. */
export interface AdapterCallOptions {
  /** The request that carries it (default `adapter.send`). */
  readonly method?: AdapterMethod;
  /** Reaches the engine for `fetch`; a test gives its own. Without it, `fetch` rejects. */
  readonly fetch?: EngineFetch;
  /** Aborted when the engine cancels the call (default: never). */
  readonly signal?: AbortSignal;
}

/** The lanes of a `fetch` (`spec/adapters.md` §2 item 5). */
export type FetchLane = "provider" | "upload" | "download";

/** The options of `call.fetch`. */
export interface FetchOptions {
  /** `GET` (the default) or `POST`. */
  readonly method?: "GET" | "POST" | undefined;
  /** An object of names to values, or `[name, value]` pairs (a `Headers`, a `Map`); names are
   * lowercased. */
  readonly headers?:
    | { readonly [name: string]: string }
    | Iterable<readonly [string, string]>
    | undefined;
  /** A JSON body, sent as compact UTF-8 with `content-type: application/json`. */
  readonly json?: JsonInput | undefined;
  /** Bytes, with their `contentType`. */
  readonly body?: Uint8Array | undefined;
  readonly contentType?: string | undefined;
  /** `provider` (the default), `upload` or `download`. */
  readonly lane?: FetchLane | undefined;
  /** Whether the engine attaches the declaration's key: only on the provider and upload lanes,
   * and only to the base URL's origin. */
  readonly credential?: boolean | undefined;
  /** The exchange's deadline in seconds, above 0 and at most 300 (the default). */
  readonly timeoutS?: number | undefined;
  /** The most response body bytes read, from 1 to 67108864 (64 MiB, the default). */
  readonly maxResponseBytes?: number | undefined;
}

/** The params of the call's methods, which are not a call's: `-32602`, with this message. */
export class NotACall extends TypeError {}

/**
 * One call an adapter is asked to send, submit or collect (module comment). `C` is the
 * capabilities it may be for, and `H` its handle: a {@link JobHandle} in `collect`, `undefined`
 * elsewhere. The host makes it from the params; a test may make one too, with a function that
 * stands in for the engine's `fetch`.
 */
export class AdapterCall<
  C extends string = string,
  H extends JobHandle | undefined = JobHandle | undefined,
> {
  readonly capability: C;
  readonly route: AdapterRoute;
  /** The declaration's base URL, normalized: its variable's value when set, else its default. */
  readonly baseUrl: string;
  /** The call key. */
  readonly key: string;
  /** The attempt, from 1 to 6. */
  readonly attempt: number;
  /** The call's take list. */
  readonly takes: readonly number[];
  /** The last of `takes`. */
  readonly take: number;
  /** The job's handle, as `submit` accepted it; `undefined` outside `collect`. */
  readonly handle: H;
  /** A file of every file value in `request`, by digest. */
  readonly files: ReadonlyMap<string, AdapterFile>;
  /** The canonical request, each file value as the {@link AdapterFile} of `files`. */
  readonly request: { readonly [member: string]: AdapterValue };
  /** Aborted when the engine cancels the call, and when the session ends. */
  readonly signal: AbortSignal;
  readonly #method: AdapterMethod;
  readonly #callId: string;
  readonly #fetch: EngineFetch | undefined;

  constructor(params: unknown, options: AdapterCallOptions = {}) {
    const method = options.method ?? "adapter.send";
    if (!Object.hasOwn(METHODS, method)) {
      throw new TypeError(`${safeString(method)} is not an adapter call`);
    }
    const refused = (what: string) => new NotACall(`${method}'s ${what}`);
    if (!isPlainObject(params)) {
      throw refused("params are a JSON object");
    }
    const callId = params.call_id;
    if (typeof callId !== "string" || !callId) {
      throw refused("call_id is a string");
    }
    const strings = (value: unknown, names: readonly string[]): value is Record<string, string> =>
      isPlainObject(value) && names.every((name) => typeof value[name] === "string");
    if (!strings(params.adapter, ["path", "attribute"])) {
      throw refused("adapter is {path, attribute}");
    }
    const capability = params.capability;
    if (typeof capability !== "string" || !capability) {
      throw refused("capability is a capability's name");
    }
    const route = params.route;
    if (!strings(route, ["id", "model", "provider"]) || !isPlainObject(route.contract)) {
      throw refused("route is {id, model, provider, contract}");
    }
    const baseUrl = params.base_url;
    if (typeof baseUrl !== "string" || !baseUrl) {
      throw refused("base_url is a URL");
    }
    const request = params.request;
    if (!isPlainObject(request)) {
      throw refused("request is a JSON object");
    }
    const files = params.files;
    if (!isPlainObject(files) || !Object.values(files).every(isAdapterFile)) {
      throw refused("files are {digest, kind, size, path} by digest");
    }
    const take = params.take;
    if (
      !Array.isArray(take) ||
      take.length === 0 ||
      !take.every((number) => Number.isInteger(number) && (number as number) >= 1)
    ) {
      throw refused("take is a list of take numbers");
    }
    const key = params.key;
    if (typeof key !== "string" || !key) {
      throw refused("key is the call key");
    }
    const attempt = params.attempt;
    if (!Number.isInteger(attempt) || (attempt as number) < 1 || (attempt as number) > 6) {
      throw refused("attempt is from 1 to 6");
    }
    const handle = params.handle;
    if (method === "adapter.collect" && !isPlainObject(handle)) {
      throw refused("handle is a JSON object");
    }
    if (method !== "adapter.collect" && Object.hasOwn(params, "handle")) {
      throw new NotACall(`${method} has no handle`);
    }
    const byDigest = new Map(
      Object.entries(files).map(([digest, file]) => [
        digest,
        new AdapterFile(file as unknown as ConstructorParameters<typeof AdapterFile>[0]),
      ]),
    );
    const takes = Object.freeze([...(take as number[])]);
    this.capability = capability as C;
    this.route = Object.freeze({
      id: route.id as string,
      model: route.model as string,
      provider: route.provider as string,
      contract: Object.freeze({ ...(route.contract as unknown as Record<string, JsonValue>) }),
    });
    this.baseUrl = baseUrl;
    this.key = key;
    this.attempt = attempt as number;
    this.takes = takes;
    this.take = takes[takes.length - 1] as number;
    const collecting = method === "adapter.collect";
    this.handle = (collecting ? Object.freeze({ ...(handle as object) }) : undefined) as H;
    this.files = byDigest;
    const missing = (digest: string) =>
      new NotACall(`the request holds the file ${digest}, which files does not hold`);
    this.request = withFiles(request, byDigest, missing) as AdapterCall["request"];
    this.signal = options.signal ?? new AbortController().signal;
    this.#method = method;
    this.#callId = callId;
    this.#fetch = options.fetch;
  }

  /** One HTTP exchange through the engine's transport (module comment): the engine attaches the
   * declaration's key when `credential` is true, on the provider and upload lanes and only to the
   * base URL's origin. */
  async fetch(url: string | URL, options: FetchOptions = {}): Promise<FetchResponse> {
    const params = fetchParams(this.#callId, url, options);
    if (this.#fetch === undefined) {
      throw new Error("this call has no engine to fetch through");
    }
    return readResponse(await this.#fetch(params));
  }

  toString(): string {
    const at = `attempt ${this.attempt}, take ${this.take}`;
    return `AdapterCall(${this.#method} ${this.capability} on ${this.route.id}, ${at})`;
  }
}

/** A call `collect` gets: its handle is the job's. */
export type CollectCall<C extends string = string> = AdapterCall<C, JobHandle>;

function isAdapterFile(file: unknown): boolean {
  if (!isPlainObject(file)) {
    return false;
  }
  const { digest, kind, size, path } = file;
  return (
    typeof digest === "string" &&
    typeof kind === "string" &&
    Number.isInteger(size) &&
    (size as number) >= 0 &&
    typeof path === "string" &&
    path !== ""
  );
}

// ------------------------------------------------------------------------------------------------
// fetch

/** The longest deadline and the largest response a `fetch` may ask for (§6.8). */
export const MAX_TIMEOUT_S = 300;
export const MAX_RESPONSE_BYTES = 64 * 1024 * 1024;
const LANES: readonly string[] = ["provider", "upload", "download"];
const PHASES: readonly string[] = ["refused", "not_sent", "after_send"];
const FETCH_OPTIONS: readonly string[] = [
  "method",
  "headers",
  "json",
  "body",
  "contentType",
  "lane",
  "credential",
  "timeoutS",
  "maxResponseBytes",
];
/** A header's name (fx-node-protocol-v1 `http_header`). */
const HEADER_NAME = /^[a-z0-9!#$%&'*+.^_`|~-]+$/;
/** Base64 with padding (RFC 4648 §4). */
const BASE64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

/** What an exchange through `fetch` answered: its `status`, its `headers` as `[name, value]`
 * pairs in order, names lowercase (`new Headers(response.headers)` reads them), and its `body`. */
export class FetchResponse {
  readonly status: number;
  readonly headers: readonly (readonly [name: string, value: string])[];
  readonly body: Uint8Array;

  constructor(status: number, headers: readonly (readonly [string, string])[], body: Uint8Array) {
    this.status = status;
    this.headers = Object.freeze(headers.map(([name, value]) => Object.freeze([name, value] as const)));
    this.body = body;
  }

  /** The body as strict UTF-8 text. */
  text(): string {
    return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(this.body);
  }

  /** The body as JSON. */
  json(): unknown {
    return JSON.parse(this.text()) as unknown;
  }
}

/** Why an exchange failed (`spec/protocol.md` §6.8). */
export type FetchPhase = "refused" | "not_sent" | "after_send";

const FETCH_ERROR = Symbol.for("grida.fx.fetch_error");

/** An exchange through `fetch` that failed: its `phase` (`refused`, `not_sent` or `after_send`,
 * which `spec/adapters.md` §3.2 maps onto an outcome) and its `reason`, which names no URL path,
 * header value or body. `instanceof` recognises one of any copy of `@grida/fx`. */
export class FetchError extends Error {
  readonly phase: FetchPhase;
  readonly reason: string;

  constructor(phase: FetchPhase, reason: string) {
    super(reason);
    this.name = "FetchError";
    this.phase = phase;
    this.reason = reason;
    Object.defineProperty(this, FETCH_ERROR, { value: true });
  }

  static [Symbol.hasInstance](value: unknown): boolean {
    return brandOf(value, FETCH_ERROR) === true;
  }
}

/** `fetch`'s params for the call `callId` (module comment). */
export function fetchParams(callId: string, url: unknown, options: FetchOptions = {}): Record<string, unknown> {
  if (!isPlainObject(options as unknown)) {
    throw new TypeError(`fetch's options are an object, not ${typeName(options)}`);
  }
  for (const [name, value] of Object.entries(options)) {
    if (value !== undefined && !FETCH_OPTIONS.includes(name)) {
      throw new TypeError(`fetch has no option ${pyRepr(name)}`);
    }
  }
  const given = url instanceof URL ? url.href : url;
  if (typeof given !== "string") {
    throw new TypeError("fetch's url is a string or a URL");
  }
  if (!given) {
    throw new TypeError("fetch's url is an absolute URL");
  }
  const method = options.method ?? "GET";
  const verb = typeof method === "string" ? method.toUpperCase() : null;
  if (verb !== "GET" && verb !== "POST") {
    throw new TypeError(`fetch takes GET or POST, not ${safeString(method)}`);
  }
  const params: Record<string, unknown> = { call_id: callId, method: verb, url: given };
  const headers = headerPairs(options.headers);
  if (headers.length > 0) {
    params.headers = headers;
  }
  const { json, body, contentType } = options;
  if (json !== undefined && body !== undefined) {
    throw new TypeError("fetch takes json or body, not both");
  }
  if (body !== undefined && !(body instanceof Uint8Array)) {
    throw new TypeError(`fetch's body is bytes, not ${typeName(body)}`);
  }
  if (body !== undefined && !(typeof contentType === "string" && contentType)) {
    throw new TypeError("a body needs its contentType");
  }
  if (body === undefined && contentType !== undefined) {
    throw new TypeError("contentType goes with a body");
  }
  if (verb === "GET" && (json !== undefined || body !== undefined)) {
    throw new TypeError("a GET has no body");
  }
  if (json !== undefined) {
    params.body = { json };
  } else if (body !== undefined) {
    const encoded = Buffer.from(body.buffer, body.byteOffset, body.byteLength).toString("base64");
    params.body = { base64: encoded, content_type: contentType };
  }
  const lane = options.lane ?? "provider";
  if (!LANES.includes(lane as string)) {
    throw new TypeError(`fetch's lane is provider, upload or download, not ${pyRepr(lane)}`);
  }
  params.lane = lane;
  const credential = options.credential ?? false;
  if (typeof credential !== "boolean") {
    throw new TypeError("credential is true or false");
  }
  if (credential) {
    params.credential = true;
  }
  const timeout = options.timeoutS;
  if (timeout !== undefined) {
    if (typeof timeout !== "number" || !Number.isFinite(timeout) || timeout <= 0 || timeout > MAX_TIMEOUT_S) {
      throw new TypeError(`timeoutS is above 0 and at most ${MAX_TIMEOUT_S}`);
    }
    params.timeout_s = timeout;
  }
  const most = options.maxResponseBytes;
  if (most !== undefined) {
    if (typeof most !== "number" || !Number.isInteger(most) || most < 1 || most > MAX_RESPONSE_BYTES) {
      throw new TypeError(`maxResponseBytes is from 1 to ${MAX_RESPONSE_BYTES}`);
    }
    params.max_response_bytes = most;
  }
  return params;
}

function headerPairs(headers: FetchOptions["headers"]): string[][] {
  if (headers === undefined) {
    return [];
  }
  const refused = () => new TypeError("fetch's headers are an object or [name, value] pairs");
  let pairs: Iterable<unknown>;
  if (isPlainObject(headers)) {
    pairs = Object.entries(headers);
  } else if (typeof headers === "object" && headers !== null && Symbol.iterator in headers) {
    pairs = headers as Iterable<unknown>;
  } else {
    throw refused();
  }
  const listed: string[][] = [];
  for (const pair of pairs) {
    if (!Array.isArray(pair) || pair.length !== 2) {
      throw refused();
    }
    const [name, value] = pair as unknown[];
    if (typeof name !== "string" || typeof value !== "string") {
      throw new TypeError("a header's name and value are strings");
    }
    const lowered = name.toLowerCase();
    if (!HEADER_NAME.test(lowered)) {
      throw new TypeError(`the header name ${JSON.stringify(name)} is not a token`);
    }
    listed.push([lowered, value]);
  }
  return listed;
}

/** `fetch`'s result: the response, or {@link FetchError} for a failed exchange. */
export function readResponse(result: unknown): FetchResponse {
  if (isPlainObject(result) && Object.keys(result).length === 1 && Object.hasOwn(result, "error")) {
    const error = result.error;
    if (isPlainObject(error) && PHASES.includes(error.phase as string) && typeof error.reason === "string") {
      throw new FetchError(error.phase as FetchPhase, error.reason);
    }
  } else if (isPlainObject(result)) {
    const { status, headers, base64 } = result;
    if (
      Number.isInteger(status) &&
      Array.isArray(headers) &&
      headers.every(
        (pair) => Array.isArray(pair) && pair.length === 2 && pair.every((part) => typeof part === "string"),
      ) &&
      typeof base64 === "string" &&
      BASE64.test(base64)
    ) {
      const body = new Uint8Array(Buffer.from(base64, "base64"));
      return new FetchResponse(status as number, headers as [string, string][], body);
    }
  }
  throw new EngineError("the engine answered fetch with something the SDK cannot read", null, INTERNAL);
}

// ------------------------------------------------------------------------------------------------
// Declaring

/** The kind of a capability an adapter serves: a request (`adapter.send`) or a job
 * (`adapter.submit`, `adapter.collect`). */
export type CallKind = "request" | "job";

/** What serves a request: a function, or an async function, of one call. */
export type SendFunction<C extends string = string> = (
  call: AdapterCall<C, undefined>,
) => SendOutcome | Promise<SendOutcome>;
/** What submits a job. */
export type SubmitFunction<C extends string = string> = (
  call: AdapterCall<C, undefined>,
) => SubmitOutcome | Promise<SubmitOutcome>;
/** What collects a job, by the handle its call carries. */
export type CollectFunction<C extends string = string> = (
  call: AdapterCall<C, JobHandle>,
) => CollectOutcome | Promise<CollectOutcome>;

/** An adapter's declaration: the capabilities it serves, the functions that serve them, and its
 * version. */
export interface AdapterDecl<R extends string = string, J extends string = string> {
  /** The capabilities `send` serves, each a request. */
  readonly requests?: readonly R[] | undefined;
  /** The capabilities `submit` and `collect` serve, each a long job. */
  readonly jobs?: readonly J[] | undefined;
  readonly send?: SendFunction<R> | undefined;
  readonly submit?: SubmitFunction<J> | undefined;
  readonly collect?: CollectFunction<J> | undefined;
  /** An integer of at least 0, which `grida-fx lock` locks; `null` (the default) for none, and
   * the adapter's identity is then its source. */
  readonly version?: number | null | undefined;
}

/** The members `O` gives a value: a union of object literals is normalized with `?: never`
 * members of its other types, which are not given. */
type Given<O> = { [K in keyof O]-?: [Exclude<O[K], undefined>] extends [never] ? never : K }[keyof O];
/** The members an outcome of `O` gives that the outcome of its name in `Takes` does not have: a
 * misspelled member (`cost_usd`), or another outcome's. */
type Stray<O, Takes> = O extends { readonly outcome: infer N }
  ? Exclude<Given<O>, keyof Extract<Takes, { readonly outcome: N }>>
  : never;
/** No stray member: TypeScript does not check a function's return for excess members, so
 * `adapter()` constrains each function's outcomes with this, as `node()` does a body's outputs. */
export type NoStray<O, Takes> = { readonly [K in Stray<O, Takes>]?: never };

/** At least one capability. */
type SomeCapability<R extends string, J extends string> = [R | J] extends [never]
  ? { readonly requests: readonly [string, ...string[]] }
  : unknown;
/** `send` for requests. */
type SendsRequests<R extends string, S> = [R] extends [never]
  ? unknown
  : { readonly send: (call: AdapterCall<R, undefined>) => S | Promise<S> };
/** `submit` and `collect` for jobs. */
type ServesJobs<J extends string, T, K> = [J] extends [never]
  ? unknown
  : {
      readonly submit: (call: AdapterCall<J, undefined>) => T | Promise<T>;
      readonly collect: (call: AdapterCall<J, JobHandle>) => K | Promise<K>;
    };

/** The brand of an adapter: what the host reads from an export (`AdapterRecord`). */
export const ADAPTER: unique symbol = Symbol.for("grida.fx.adapter") as never;

/** What an adapter's brand holds. */
export interface AdapterRecord {
  /** Each capability and its kind, in the order the adapter declares them. */
  readonly capabilities: { readonly [capability: string]: CallKind };
  readonly version: number | null;
  readonly send: ((call: never) => unknown) | undefined;
  readonly submit: ((call: never) => unknown) | undefined;
  readonly collect: ((call: never) => unknown) | undefined;
}

/** A declared adapter, made by {@link adapter}. Export it: a declaration names it by its module
 * and export (`adapter: adapters/acme.ts#acme`). */
export interface Adapter<R extends string = string, J extends string = string> {
  readonly requests: readonly R[];
  readonly jobs: readonly J[];
  /** Each capability and its kind, `request` or `job`, in the order it declares them. */
  readonly capabilities: { readonly [capability: string]: CallKind };
  readonly version: number | null;
  send?(call: AdapterCall<R, undefined>): SendOutcome | Promise<SendOutcome>;
  submit?(call: AdapterCall<J, undefined>): SubmitOutcome | Promise<SubmitOutcome>;
  collect?(call: AdapterCall<J, JobHandle>): CollectOutcome | Promise<CollectOutcome>;
  readonly [ADAPTER]: AdapterRecord;
}

/** A capability's name (fx-node-protocol-v1). */
const CAPABILITY = /^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*$/;
const DECL_FIELDS: readonly string[] = ["requests", "jobs", "send", "submit", "collect", "version"];

/**
 * Declares a project adapter (module comment): `send` serves each capability of `requests`, and
 * `submit` and `collect` each of `jobs`. A module exports it under the name a declaration's
 * `adapter: <path>#<name>` gives. Each function's call is typed with the capabilities it serves.
 */
export function adapter<
  const R extends string = never,
  const J extends string = never,
  S extends SendOutcome & NoStray<S, SendOutcome> = SendOutcome,
  T extends SubmitOutcome & NoStray<T, SubmitOutcome> = SubmitOutcome,
  K extends CollectOutcome & NoStray<K, CollectOutcome> = CollectOutcome,
>(
  decl: AdapterDecl<R, J> & SomeCapability<R, J> & SendsRequests<R, S> & ServesJobs<J, T, K>,
): Adapter<R, J> {
  if (!isPlainObject(decl)) {
    throw new TypeError(`an adapter declaration is an object, not ${typeName(decl)}`);
  }
  for (const [field, value] of Object.entries(decl)) {
    if (value !== undefined && !DECL_FIELDS.includes(field)) {
      throw new TypeError(`an adapter declaration has no field ${pyRepr(field)}`);
    }
  }
  const named: string[] = [];
  const lists: Record<"requests" | "jobs", string[]> = { requests: [], jobs: [] };
  for (const member of ["requests", "jobs"] as const) {
    const value: unknown = decl[member];
    if (value === undefined) {
      continue;
    }
    if (typeof value !== "object" || value === null || !(Symbol.iterator in value)) {
      throw new TypeError(`adapter(): ${member} is a list of capability names`);
    }
    for (const name of value as Iterable<unknown>) {
      if (typeof name !== "string") {
        throw new TypeError(`adapter(): ${member} is a list of capability names`);
      }
      if (!CAPABILITY.test(name)) {
        throw new TypeError(`adapter(): ${JSON.stringify(name)} is not a capability name`);
      }
      if (named.includes(name)) {
        throw new TypeError(`adapter(): ${name} is named twice`);
      }
      named.push(name);
      lists[member].push(name);
    }
  }
  if (named.length === 0) {
    throw new TypeError("adapter() declares no capability: give it requests or jobs");
  }
  const functions: Record<string, ((call: never) => unknown) | undefined> = {};
  for (const member of ["send", "submit", "collect"] as const) {
    const value: unknown = decl[member];
    if (value !== undefined && typeof value !== "function") {
      throw new TypeError(`adapter(): ${member} is a function`);
    }
    functions[member] = value as ((call: never) => unknown) | undefined;
  }
  if (lists.requests.length > 0 && functions.send === undefined) {
    throw new TypeError("adapter(): requests need send");
  }
  if (lists.jobs.length > 0 && (functions.submit === undefined || functions.collect === undefined)) {
    throw new TypeError("adapter(): jobs need submit and collect");
  }
  const version: unknown = decl.version ?? null;
  if (version !== null && !(Number.isSafeInteger(version) && (version as number) >= 0)) {
    throw new TypeError("adapter(): version is an integer of at least 0, or null");
  }
  const capabilities: Record<string, CallKind> = {};
  for (const name of lists.requests) {
    capabilities[name] = "request";
  }
  for (const name of lists.jobs) {
    capabilities[name] = "job";
  }
  Object.freeze(capabilities);
  const record: AdapterRecord = Object.freeze({
    capabilities,
    version: version as number | null,
    send: functions.send,
    submit: functions.submit,
    collect: functions.collect,
  });
  const made: Record<string, unknown> = {
    requests: Object.freeze([...lists.requests]),
    jobs: Object.freeze([...lists.jobs]),
    capabilities,
    version: record.version,
  };
  for (const member of ["send", "submit", "collect"] as const) {
    if (functions[member] !== undefined) {
      made[member] = functions[member];
    }
  }
  Object.defineProperty(made, ADAPTER, { value: record });
  return Object.freeze(made) as unknown as Adapter<R, J>;
}

/** The adapter an export holds, by its brand (any copy of `@grida/fx`); undefined for anything
 * else. */
export function adapterRecordOf(value: unknown): AdapterRecord | undefined {
  const record = brandOf(value, ADAPTER);
  if (typeof record !== "object" || record === null) {
    return undefined;
  }
  const { capabilities, version, send, submit, collect } = record as Record<string, unknown>;
  const fn = (given: unknown) => given === undefined || typeof given === "function";
  if (
    !isPlainObject(capabilities) ||
    !Object.values(capabilities).every((kind) => kind === "request" || kind === "job") ||
    !(version === null || (Number.isSafeInteger(version) && (version as number) >= 0)) ||
    !fn(send) ||
    !fn(submit) ||
    !fn(collect)
  ) {
    return undefined;
  }
  return record as AdapterRecord;
}

/** Each method's function, and the kind of capability it serves (§5.8). */
const METHODS: Readonly<Record<AdapterMethod, readonly ["send" | "submit" | "collect", CallKind]>> = {
  "adapter.send": ["send", "request"],
  "adapter.submit": ["submit", "job"],
  "adapter.collect": ["collect", "job"],
};

/** The methods an adapter host answers for a call. */
export const ADAPTER_METHODS: readonly AdapterMethod[] = Object.keys(METHODS) as AdapterMethod[];

/** The function that answers `method` for `capability`, when the adapter serves the capability
 * with it. */
export function functionFor(
  record: AdapterRecord,
  method: AdapterMethod,
  capability: string,
): ((call: never) => unknown) | undefined {
  const [name, kind] = METHODS[method];
  if (!Object.hasOwn(record.capabilities, capability) || record.capabilities[capability] !== kind) {
    return undefined;
  }
  return record[name];
}

// ------------------------------------------------------------------------------------------------
// Sending an outcome

/** The outcomes each method takes, in the order a refusal names them. */
const TAKES: Readonly<Record<AdapterMethod, readonly string[]>> = {
  "adapter.send": ["answered", "not_received", "refused", "failed"],
  "adapter.submit": ["accepted", "uncertain", "not_received", "refused", "failed"],
  "adapter.collect": ["answered", "ended", "unreachable"],
};
/** Each outcome's type name, and its members. */
const OUTCOMES: Readonly<Record<string, readonly [string, readonly string[]]>> = {
  answered: ["Answered", ["files", "data", "costUsd", "usage"]],
  not_received: ["NotReceived", ["reason", "retryAfterS"]],
  refused: ["Refused", ["reason"]],
  failed: ["Failed", ["reason", "retryable", "costUsd"]],
  accepted: ["Accepted", ["handle"]],
  uncertain: ["Uncertain", ["reason"]],
  ended: ["Ended", ["reason", "costUsd"]],
  unreachable: ["Unreachable", ["reason"]],
};
const USAGE: Readonly<Record<string, string>> = {
  inputTokens: "input_tokens",
  cachedInputTokens: "cached_input_tokens",
  outputTokens: "output_tokens",
};

/** What a function returned as `method`'s result (module comment); a `TypeError` for anything the
 * SDK cannot send. */
export async function encodeOutcome(
  method: AdapterMethod,
  returned: unknown,
): Promise<Record<string, unknown>> {
  const takes = TAKES[method];
  const outcome = isPlainObject(returned) ? returned.outcome : undefined;
  if (typeof outcome !== "string" || !takes.includes(outcome)) {
    const names = takes.map((taken) => (OUTCOMES[taken] as readonly [string, unknown])[0]);
    const known = typeof outcome === "string" && Object.hasOwn(OUTCOMES, outcome);
    const given = known
      ? (OUTCOMES[outcome] as readonly [string, unknown])[0]
      : typeof outcome === "string"
        ? `the outcome ${pyRepr(outcome)}`
        : typeName(returned);
    const fn = (METHODS[method] as readonly [string, unknown])[0];
    throw new TypeError(`${fn} returns ${names.slice(0, -1).join(", ")} or ${names.at(-1)}, not ${given}`);
  }
  const given = returned as Record<string, unknown>;
  const [name, members] = OUTCOMES[outcome] as readonly [string, readonly string[]];
  for (const [member, value] of Object.entries(given)) {
    if (value !== undefined && member !== "outcome" && !members.includes(member)) {
      const all = ["outcome", ...members];
      const listed = `${all.slice(0, -1).join(", ")} and ${all.at(-1)}`;
      throw new TypeError(`${name} holds ${listed}, not ${pyRepr(member)}`);
    }
  }
  const wire: Record<string, unknown> = { outcome };
  if (members.includes("reason")) {
    wire.reason = reasonOf(given.reason);
  }
  if (outcome === "answered") {
    wire.files = await encodeFiles(given.files, ADAPTER_FILES);
    wire.data = encodeData(given.data);
  }
  if (outcome === "not_received") {
    setAmount(wire, "retry_after_s", "retryAfterS", given.retryAfterS);
  }
  if (outcome === "failed") {
    if (typeof given.retryable !== "boolean") {
      throw new TypeError("retryable is true or false");
    }
    wire.retryable = given.retryable;
  }
  if (members.includes("costUsd")) {
    setAmount(wire, "cost_usd", "costUsd", given.costUsd);
  }
  if (outcome === "answered" && given.usage !== undefined) {
    wire.usage = usageOf(given.usage);
  }
  if (outcome === "accepted") {
    wire.handle = handleOf(given.handle);
  }
  return wire;
}

function reasonOf(value: unknown): string {
  if (typeof value !== "string") {
    throw new TypeError(`a reason is a sentence, not ${typeName(value)}`);
  }
  if (!value.trim()) {
    throw new TypeError("a reason is a sentence, not a blank string");
  }
  return value;
}

function setAmount(wire: Record<string, unknown>, member: string, name: string, value: unknown): void {
  if (value === undefined) {
    return;
  }
  if (typeof value !== "number") {
    throw new TypeError(`${name} is a number, not ${typeName(value)}`);
  }
  if (!Number.isFinite(value) || value < 0) {
    throw new TypeError(`${name} is a number of at least 0, not ${String(value)}`);
  }
  wire[member] = value;
}

function usageOf(usage: unknown): Record<string, number> {
  if (!isPlainObject(usage)) {
    throw new TypeError(`usage is an object of token counts, not ${typeName(usage)}`);
  }
  const counts: Record<string, number> = {};
  for (const [name, count] of Object.entries(usage)) {
    if (count === undefined) {
      continue;
    }
    const member = USAGE[name];
    if (member === undefined || !Object.hasOwn(USAGE, name)) {
      throw new TypeError(`usage counts ${Object.keys(USAGE).join(", ")}, not ${pyRepr(name)}`);
    }
    if (!Number.isSafeInteger(count) || (count as number) < 0) {
      throw new TypeError(`usage ${name} is an integer of at least 0, not ${pyRepr(count)}`);
    }
    counts[member] = count as number;
  }
  return counts;
}

function handleOf(handle: unknown): unknown {
  if (!isPlainObject(handle)) {
    throw new TypeError(`a handle is an object, not ${typeName(handle)}`);
  }
  try {
    // The handle sits in the result of a response: two levels inside its message.
    checkValue(handle, "/handle", 2);
  } catch (error) {
    throw new TypeError(`a handle is not a JSON object FX can read: ${(error as Error).message}`);
  }
  return handle;
}

// ------------------------------------------------------------------------------------------------
// Serving

/** One answer to a call. */
export type AdapterReply = { readonly result: unknown } | { readonly error: ErrorBody };

/** A call being answered. */
interface Serving {
  readonly method: AdapterMethod;
  readonly controller: AbortController;
  readonly reply: (reply: AdapterReply) => void;
}

/** Options of an {@link AdapterServer}. */
export interface AdapterServerOptions {
  /** How a fault is named in its answer (default {@link errorText}). */
  readonly errorText?: (error: unknown) => string;
  /** Told of each fault, with the adapter's name (`<path>#<attribute>`). */
  readonly onFault?: (error: unknown, name: string) => void;
}

/** What {@link AdapterServer.start} answers a call with. */
export interface AdapterWork {
  readonly method: AdapterMethod;
  /** The adapter's function for the method. */
  readonly fn: (call: never) => unknown;
  /** Made with `controller`'s signal. */
  readonly call: AdapterCall;
  readonly controller: AbortController;
  /** `<path>#<attribute>`. */
  readonly name: string;
}

/** Answers adapter calls (module comment). */
export class AdapterServer {
  readonly #errorText: (error: unknown) => string;
  readonly #onFault: ((error: unknown, name: string) => void) | undefined;
  /** The calls not answered yet, by request id. */
  readonly #serving = new Map<Id, Serving>();

  constructor(options: AdapterServerOptions = {}) {
    this.#errorText = options.errorText ?? errorText;
    this.#onFault = options.onFault;
  }

  /** How many calls are being answered. */
  get pending(): number {
    return this.#serving.size;
  }

  /** Starts answering one call. `reply` gets `{result}` or `{error}` once, unless {@link cancel}
   * or {@link close} ends the call first. */
  start(id: Id, work: AdapterWork, reply: (reply: AdapterReply) => void): void {
    const serving: Serving = { method: work.method, controller: work.controller, reply };
    this.#serving.set(id, serving);
    void this.#answer(id, serving, work);
  }

  /** `$/cancel`: answers the call `cancelled` and aborts its signal; false when `id` names no call
   * being answered. */
  cancel(id: Id): boolean {
    const serving = this.#serving.get(id);
    if (serving === undefined) {
      return false;
    }
    this.#serving.delete(id);
    const message = `the engine cancelled ${serving.method}`;
    serving.reply({ error: { code: CANCELLED, message } });
    serving.controller.abort(new Cancelled(message));
    return true;
  }

  /** Ends every call being answered, answering none of them: the session is over. */
  close(): void {
    const serving = [...this.#serving.values()];
    this.#serving.clear();
    for (const entry of serving) {
      entry.controller.abort(new Cancelled("the engine ended the session"));
    }
  }

  async #answer(id: Id, serving: Serving, work: AdapterWork): Promise<void> {
    let reply: AdapterReply;
    try {
      // Called after this turn of the loop, so a function that throws at once is answered too.
      await Promise.resolve();
      const returned = await (work.fn as (call: AdapterCall) => unknown)(work.call);
      if (this.#serving.get(id) !== serving) {
        return; // cancelled meanwhile: what it returned is discarded
      }
      reply = { result: await encodeOutcome(work.method, returned) };
    } catch (error) {
      if (this.#serving.get(id) !== serving) {
        return; // thrown after the engine cancelled the call: nobody waits for it
      }
      try {
        this.#onFault?.(error, work.name);
      } catch {
        // telling of the fault failed: the call is still answered
      }
      reply = { error: { code: INTERNAL, message: encodableText(this.#errorText(error)) } };
    }
    if (this.#serving.get(id) !== serving) {
      return;
    }
    this.#serving.delete(id);
    serving.reply(reply);
  }
}
