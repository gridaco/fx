/**
 * Stand-ins (`spec/protocol.md` §5.7 and §8.2): what the SDK's two answerers share.
 *
 * A stand-in is a function, or an async function, of one {@link StandInCall} that answers the paid
 * calls of a stand-in run in place of a provider. `run(target, { standIn })` serves it in the
 * caller's own process, over a socket on the engine's standard input ({@link serveStandIn});
 * `grida-fx run --stand-in file.ts#answer` serves it from a stand-in host (`host.ts`). Both answer
 * `stand_in.answer` through an {@link Answerer}:
 *
 * - **The call.** The params become a {@link StandInCall}: `capability`, `route` (`id`,
 *   `fingerprint`), `key`, `takes` (the `take` list) and `take` (its last number, as
 *   `ctx.instance.take`), `instance` (`id`, `path`, `step`), `files` (an `InputFile` by digest),
 *   `request` (a copy of the request in which every file value, an object of exactly the shape
 *   `{"file": <digest>}` at any depth, is the `InputFile` of `files`), `params` (the params as they
 *   came) and `signal`, aborted when the engine cancels the call. Params of another shape, or a
 *   file value `files` does not hold, are answered `-32602`.
 * - **One at a time.** Calls are answered one at a time, in the order their requests came. A
 *   function that never yields holds the process; one that ignores `call.signal` holds the calls
 *   after it.
 * - **The answer.** An {@link Answer} goes out as `{files: {name: {base64, kind?}}, data}`: a
 *   `Uint8Array` leaves `kind` out (the engine gives the file the kind its capability names), an
 *   `InputFile` gives its own kind, `{bytes, kind}` its `kind`, and a `file:` URL its bytes with the
 *   kind of its suffix (`spec/identity.md` §4); a file whose value is `undefined` is left out.
 *   {@link DECLINE} goes out as `{"decline": true}`. A thrown `CallRefused` is answered
 *   `capability_refused`, and a `CallFailed` `call_failed`, each with its message.
 * - **Faults.** Anything else the function throws, a return value that is neither an answer nor
 *   `DECLINE` (`undefined` too), and an answer the SDK cannot send (a file that is not bytes, a file
 *   name that is not `[a-z][a-z0-9_]*`, `data` the engine cannot read) are faults of the stand-in:
 *   answered `internal` with `<name>: <message>`, which stops the run. The first fault is kept as
 *   {@link Answerer.fault}, and every later call is answered `internal` (`the stand-in failed
 *   earlier`) without calling the function.
 * - **Cancelling.** `$/cancel` answers the call `cancelled` at once and aborts its signal; a call
 *   still waiting for its turn is never asked. What the function returns or throws after that is
 *   dropped, never a fault.
 * - **Sending.** Every error answer can be sent: a lone surrogate in its message is written as its
 *   escape (`\udcff`), so no call goes unanswered.
 */

import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import type { Socket } from "node:net";
import { fileURLToPath } from "node:url";
import { CallFailed, CallRefused, isEngineError } from "./engine-errors.js";
import { adapterFileOf, DIGEST, type FileRefWire, fileRefOf, InputFile } from "./files.js";
import { kindOf } from "./kinds.js";
import {
  CANCELLED,
  checkValue,
  type ErrorBody,
  encodableText,
  FrameReader,
  frame,
  type Id,
  INTERNAL,
  INVALID_PARAMS,
  INVALID_REQUEST,
  isPlainObject,
  METHOD_NOT_FOUND,
  PARSE_ERROR,
  PROTOCOL,
  PROTOCOL_MISMATCH,
  parseMessage,
  pyRepr,
  Refusal,
  safeString,
  setOwn,
  typeName,
  validId,
} from "./protocol.js";
import type { JsonInput, JsonValue } from "./workflow.js";

// ------------------------------------------------------------------------------------------------
// The call

/** One paid call a stand-in is asked to answer (module comment). */
export interface StandInCall {
  readonly capability: string;
  /** The route's id (`model@provider`) and its fingerprint. */
  readonly route: { readonly id: string; readonly fingerprint: string };
  /** The call key. */
  readonly key: string;
  /** The call's take list. */
  readonly takes: readonly number[];
  /** The last of `takes`. */
  readonly take: number;
  /** The instance making the call. */
  readonly instance: { readonly id: string; readonly path: string; readonly step: string };
  /** A file of every file value in `request`, by digest. */
  readonly files: ReadonlyMap<string, InputFile>;
  /** The canonical request, each file value as the `InputFile` of `files`. */
  readonly request: { readonly [member: string]: StandInValue };
  /** The params as they came. */
  readonly params: { readonly [name: string]: JsonValue };
  /** Aborted when the engine cancels the call (the instance making it timed out or was
   * stopped), and when the session ends. */
  readonly signal: AbortSignal;
}

/** A value of a call's request: JSON, with files as `InputFile` objects. */
export type StandInValue =
  | null
  | boolean
  | number
  | string
  | InputFile
  | readonly StandInValue[]
  | { readonly [key: string]: StandInValue };

/** A stand-in: a function, or an async function, of one call. */
export type StandIn = (call: StandInCall) => Answer | Decline | Promise<Answer | Decline>;

/** `stand_in.answer`'s params that are not a call's: `-32602`, with this message. */
class NotACall extends Error {}

/** The call `stand_in.answer`'s params ask for; {@link NotACall} for params of another shape. */
function callOf(params: unknown, signal: AbortSignal): StandInCall {
  if (!isPlainObject(params)) {
    throw new NotACall("stand_in.answer's params are a JSON object");
  }
  const capability = params.capability;
  if (typeof capability !== "string" || !capability) {
    throw new NotACall("stand_in.answer's capability is a capability's name");
  }
  const route = params.route;
  const strings = (value: Record<string, unknown>, names: readonly string[]) =>
    names.every((name) => typeof value[name] === "string");
  if (!isPlainObject(route) || !strings(route, ["id", "fingerprint"])) {
    throw new NotACall("stand_in.answer's route is {id, fingerprint}");
  }
  const take = params.take;
  if (
    !Array.isArray(take) ||
    take.length === 0 ||
    !take.every((number) => typeof number === "number" && Number.isInteger(number) && number >= 1)
  ) {
    throw new NotACall("stand_in.answer's take is a list of take numbers");
  }
  const key = params.key;
  if (typeof key !== "string" || !key) {
    throw new NotACall("stand_in.answer's key is the call key");
  }
  const instance = params.instance;
  if (!isPlainObject(instance) || !strings(instance, ["id", "path", "step"])) {
    throw new NotACall("stand_in.answer's instance is {id, path, step}");
  }
  const files = params.files;
  const isRef = (ref: unknown) => isPlainObject(ref) && typeof ref.path === "string";
  if (!isPlainObject(files) || !Object.values(files).every(isRef)) {
    throw new NotACall("stand_in.answer's files are file refs by digest");
  }
  const request = params.request;
  if (!isPlainObject(request)) {
    throw new NotACall("stand_in.answer's request is a JSON object");
  }
  const byDigest = new Map(
    Object.entries(files).map(([digest, ref]) => [
      digest,
      new InputFile(ref as unknown as FileRefWire),
    ]),
  );
  const takes = Object.freeze([...(take as number[])]);
  return Object.freeze({
    capability,
    route: Object.freeze({ id: route.id as string, fingerprint: route.fingerprint as string }),
    key,
    takes,
    take: takes[takes.length - 1] as number,
    instance: Object.freeze({
      id: instance.id as string,
      path: instance.path as string,
      step: instance.step as string,
    }),
    files: byDigest,
    request: withFiles(request, byDigest, refusedFile) as StandInCall["request"],
    params: Object.freeze({ ...params }) as StandInCall["params"],
    signal,
  });
}

/** Params that hold a file value their `files` do not hold. */
function refusedFile(digest: string): Error {
  return new NotACall(`the request holds the file ${digest}, which files does not hold`);
}

/** A copy of a JSON value with every file value (an object whose only member is `file`, holding a
 * digest) as the file of its digest in `files`; `missing` makes the error for a digest `files`
 * does not hold. A stand-in's call gets `InputFile` objects, and an adapter's `AdapterFile`
 * objects. */
export function withFiles(
  value: unknown,
  files: ReadonlyMap<string, unknown>,
  missing: (digest: string) => Error,
): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => withFiles(item, files, missing));
  }
  if (isPlainObject(value)) {
    const keys = Object.keys(value);
    const digest = value.file;
    if (keys.length === 1 && typeof digest === "string" && DIGEST.test(digest)) {
      if (!files.has(digest)) {
        throw missing(digest);
      }
      return files.get(digest);
    }
    const copy: Record<string, unknown> = {};
    for (const key of keys) {
      setOwn(copy, key, withFiles(value[key], files, missing));
    }
    return copy;
  }
  return value;
}

// ------------------------------------------------------------------------------------------------
// The answer

declare const decline: unique symbol;

/** The type of {@link DECLINE}: its unique symbol, branded so that an async stand-in whose only
 * answer is `DECLINE` keeps the type (TypeScript widens a lone unique symbol an async function
 * returns to `symbol`). */
export type Decline = typeof decline & { readonly "grida.fx.decline"?: never };

/** What a stand-in returns to decline a call: it goes on as in a run without a stand-in, and
 * fails `not_live` (`spec/protocol.md` §6.1). It is `Symbol.for("grida.fx.decline")`, so another
 * copy of `@grida/fx` declines the same way. */
export const DECLINE: Decline = Symbol.for("grida.fx.decline") as Decline;

/** Whether `value` is {@link DECLINE} (of any copy of `@grida/fx`): `r === DECLINE` does not narrow
 * an `Answer | Decline`, so a stand-in that wraps another tells the two apart with this. */
export function isDecline(value: unknown): value is Decline {
  return value === DECLINE;
}

/** A file of an answer: bytes (the kind the capability names), an `InputFile` (its kind),
 * `{bytes, kind}`, or a `file:` URL (the kind of its suffix). */
export type AnswerFile =
  | Uint8Array
  | InputFile
  | { readonly bytes: Uint8Array; readonly kind?: string }
  | URL;

/** A stand-in's answer: `files` by name (`[a-z][a-z0-9_]*`) and `data`, a JSON value (`null`
 * when left out). */
export interface Answer {
  readonly files?: { readonly [name: string]: AnswerFile };
  readonly data?: JsonInput;
}

/** A tool call in an `agent.turn` answer: a call without an `id` is `call_<n>`, counted from 1 in
 * the turn. */
export interface TurnCall {
  readonly name: string;
  readonly arguments?: { readonly [name: string]: JsonInput | undefined };
  readonly id?: string;
}

/** Answers of the usual shapes. */
export const Answer = {
  /** A `structured.generate` answer: `data` of `{json: value}`. */
  json(value: JsonInput): Answer {
    return { data: { json: value } };
  },
  /** An `agent.turn` answer (`spec/protocol.md` §6.2): `text`, and the tool calls. */
  turn(text = "", toolCalls: readonly TurnCall[] = []): Answer {
    const calls = toolCalls.map((call, index) => ({
      id: call.id || `call_${index + 1}`,
      name: call.name,
      arguments: { ...(call.arguments ?? {}) },
    }));
    return { data: { text, tool_calls: calls } };
  },
  /** An `agent.turn` answer that calls `submit` with `args`. */
  submit(args: { readonly [name: string]: JsonInput | undefined } = {}): Answer {
    return Answer.turn("", [{ name: "submit", arguments: args }]);
  },
};

/** A file's name in an answer (fx-node-protocol-v1 `stand_in_answer_result`). */
const FILE_NAME = /^[a-z][a-z0-9_]*$/;
const ANSWER_FIELDS = ["files", "data"];

/** What a stand-in returned as `stand_in.answer`'s result. A `TypeError` for anything the SDK
 * cannot send (module comment). */
export async function encode(returned: unknown): Promise<Record<string, unknown>> {
  if (returned === DECLINE) {
    return { decline: true };
  }
  if (!isPlainObject(returned)) {
    throw new TypeError(`a stand-in returns an answer or DECLINE, not ${typeName(returned)}`);
  }
  for (const key of Object.keys(returned)) {
    if (!ANSWER_FIELDS.includes(key)) {
      throw new TypeError(`an answer holds files and data, not ${pyRepr(key)}`);
    }
  }
  const files = await encodeFiles(returned.files, STAND_IN_FILES);
  return { files, data: encodeData(returned.data) };
}

/** What an answer's file may be beside bytes, `{bytes, kind}` and a file: URL: a stand-in's
 * takes an `InputFile` of its call, and an adapter's an `AdapterFile` of its call. `refusal`
 * names them all. */
export interface AnswerFiles {
  readonly fileOf: (value: unknown) => { readonly path: string; readonly kind: string } | undefined;
  readonly refusal: string;
}

export const STAND_IN_FILES: AnswerFiles = {
  fileOf: (value) => {
    const ref = fileRefOf(value);
    return ref === undefined ? undefined : { path: ref.path, kind: String(ref.kind) };
  },
  refusal: "bytes, an InputFile, {bytes, kind} or a file: URL",
};

export const ADAPTER_FILES: AnswerFiles = {
  fileOf: adapterFileOf,
  refusal: "bytes, an AdapterFile, {bytes, kind} or a file: URL",
};

/** An answer's `files` as `{name: {base64, kind?}}` (none when left out, and a file whose value is
 * `undefined` left out); a `TypeError` for files the SDK cannot send, which names what a file may
 * be (`accepted`). */
export async function encodeFiles(
  given: unknown,
  accepted: AnswerFiles,
): Promise<Record<string, unknown>> {
  const named = given ?? {};
  if (!isPlainObject(named)) {
    throw new TypeError(
      `an answer's files are an object of names to files, not ${typeName(named)}`,
    );
  }
  const files: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(named)) {
    if (value === undefined) {
      continue;
    }
    if (!FILE_NAME.test(name)) {
      throw new TypeError(
        `an answer's files are named in lowercase ([a-z][a-z0-9_]*), not ${pyRepr(name)}`,
      );
    }
    files[name] = await encodeFile(name, value, accepted);
  }
  return files;
}

/** An answer's `data` (`null` when left out); a `TypeError` for a value the engine cannot read. */
export function encodeData(given: unknown): unknown {
  const data = given ?? null;
  try {
    // The data sits in the result of a response: two levels inside its message.
    checkValue(data, "/data", 2);
  } catch (error) {
    const why = (error as Error).message;
    throw new TypeError(`an answer's data is not a JSON value FX can read: ${why}`);
  }
  return data;
}

async function encodeFile(
  name: string,
  value: unknown,
  accepted: AnswerFiles,
): Promise<Record<string, unknown>> {
  if (value instanceof Uint8Array) {
    return { base64: base64(value) };
  }
  const file = accepted.fileOf(value);
  if (file !== undefined) {
    return { base64: base64(await readFile(file.path)), kind: file.kind };
  }
  if (value instanceof URL) {
    if (value.protocol !== "file:") {
      // Only the scheme: a URL may carry credentials or a signature.
      const scheme = value.protocol;
      throw new TypeError(`${name} is ${SAID_AN.test(scheme) ? "an" : "a"} ${scheme} URL, not a file: URL`);
    }
    const path = fileURLToPath(value);
    return { base64: base64(await readFile(path)), kind: kindOf(path) };
  }
  if (isPlainObject(value) && value.bytes instanceof Uint8Array) {
    const kind = value.kind;
    if (kind === undefined) {
      return { base64: base64(value.bytes) };
    }
    if (typeof kind !== "string" || !kind) {
      throw new TypeError(`an answer's file kind is a file kind; ${name}'s is ${pyRepr(kind)}`);
    }
    return { base64: base64(value.bytes), kind };
  }
  throw new TypeError(`an answer's file is ${accepted.refusal}; ${name} is ${typeName(value)}`);
}

/** Schemes said with "an" before them: those that start with a vowel, and those said letter by
 * letter whose first letter is (an https: URL, an s3: URL). */
const SAID_AN = /^(?:[aeiou]|(?:https?|ftps?|sftp|ssh|smb|nfs|s3):)/i;

/** Bytes as base64 with padding. */
export function base64(data: Uint8Array): string {
  return Buffer.from(data.buffer, data.byteOffset, data.byteLength).toString("base64");
}

// ------------------------------------------------------------------------------------------------
// Answering

/** One answer to a request. */
export type Reply = { readonly result: unknown } | { readonly error: ErrorBody };

/** `<name>: <message>`; the name alone when the message is empty; `NonError: <value>` for a
 * thrown value that is not an error. */
export function errorText(error: unknown): string {
  if (error instanceof Error || (typeof error === "object" && error !== null && "message" in error)) {
    const { name, message } = error as { name?: unknown; message?: unknown };
    const head = typeof name === "string" && name ? name : "Error";
    const text = safeString(message ?? "");
    return text ? `${head}: ${text}` : head;
  }
  const text = safeString(error);
  return text ? `NonError: ${text}` : "NonError";
}

const CANCELLED_MESSAGE = "the engine cancelled stand_in.answer";

/** A call being answered. */
interface Asked {
  readonly controller: AbortController;
  readonly reply: (reply: Reply) => void;
}

/** Options of an {@link Answerer}. */
export interface AnswererOptions {
  /** How a fault is named in its answer (default {@link errorText}). */
  readonly errorText?: (error: unknown) => string;
  /** Told of the first fault. */
  readonly onFault?: (error: unknown) => void;
}

/** Answers `stand_in.answer` for one stand-in (module comment). */
export class Answerer {
  readonly function: StandIn;
  #fault: { readonly error: unknown } | null = null;
  readonly #errorText: (error: unknown) => string;
  readonly #onFault: ((error: unknown) => void) | undefined;
  /** Settles when the call before the next one is done: calls take turns. */
  #turn: Promise<void> = Promise.resolve();
  /** The calls not answered yet, by request id. */
  readonly #asked = new Map<Id, Asked>();

  constructor(fn: StandIn, options: AnswererOptions = {}) {
    if (typeof fn !== "function") {
      throw new TypeError(`a stand-in is a function of one StandInCall, not ${typeName(fn)}`);
    }
    this.function = fn;
    this.#errorText = options.errorText ?? errorText;
    this.#onFault = options.onFault;
  }

  /** The first fault of the stand-in (what it threw, or why its answer could not be sent), kept
   * for the caller to throw; null while there is none. */
  get fault(): { readonly error: unknown } | null {
    return this.#fault;
  }

  /** Starts answering one `stand_in.answer`. `reply` gets `{result}` or `{error}` once, unless
   * {@link close} ends the call first: at once for params that are not a call's. */
  start(id: Id, params: unknown, reply: (reply: Reply) => void): void {
    const controller = new AbortController();
    let call: StandInCall;
    try {
      call = callOf(params, controller.signal);
    } catch (error) {
      if (error instanceof NotACall) {
        reply({ error: { code: INVALID_PARAMS, message: encodableText(error.message) } });
        return;
      }
      throw error;
    }
    const asked: Asked = { controller, reply };
    this.#asked.set(id, asked);
    const turn = this.#turn.then(() => this.#answer(id, asked, call));
    this.#turn = turn.catch(() => undefined);
  }

  /** `$/cancel`: answers the call `cancelled` and aborts its signal; false when `id` names no call
   * being answered. */
  cancel(id: Id): boolean {
    const asked = this.#asked.get(id);
    if (asked === undefined) {
      return false;
    }
    this.#asked.delete(id);
    asked.reply({ error: { code: CANCELLED, message: CANCELLED_MESSAGE } });
    asked.controller.abort(new Error(CANCELLED_MESSAGE));
    return true;
  }

  /** Ends every call being answered, answering none of them: the session is over. */
  close(): void {
    const asked = [...this.#asked.values()];
    this.#asked.clear();
    for (const entry of asked) {
      entry.controller.abort(new Error("the engine ended the session"));
    }
  }

  async #answer(id: Id, asked: Asked, call: StandInCall): Promise<void> {
    if (this.#asked.get(id) !== asked) {
      return; // cancelled while it waited, or the session ended
    }
    if (this.#fault !== null) {
      this.#settle(id, asked, { error: { code: INTERNAL, message: "the stand-in failed earlier" } });
      return;
    }
    let reply: Reply;
    try {
      reply = { result: await encode(await this.function(call)) };
    } catch (error) {
      if (this.#asked.get(id) !== asked) {
        return; // thrown after the engine cancelled the call: nobody waits for it
      }
      if (isEngineError(error) && (error.code === CallRefused.code || error.code === CallFailed.code)) {
        const message = safeString(error.message) || String(error.name || "Error");
        reply = { error: { code: error.code, message: encodableText(message) } };
      } else {
        reply = { error: { code: INTERNAL, message: encodableText(this.#faulted(error)) } };
      }
    }
    this.#settle(id, asked, reply);
  }

  /** Sends `reply` unless the call was cancelled meanwhile. */
  #settle(id: Id, asked: Asked, reply: Reply): void {
    if (this.#asked.get(id) !== asked) {
      return;
    }
    this.#asked.delete(id);
    asked.reply(reply);
  }

  /** Keeps the first fault; how its answer names it. */
  #faulted(error: unknown): string {
    if (this.#fault === null) {
      this.#fault = { error };
      try {
        this.#onFault?.(error);
      } catch {
        // telling of the fault failed: the call is still answered
      }
    }
    return this.#errorText(error);
  }
}

let version: string | undefined;

/** This package's version, from its manifest (read where the compiled module is, so the host and
 * the SDK name the copy that answers). */
export function sdkVersion(): string {
  if (version === undefined) {
    try {
      const manifest = createRequire(import.meta.url)("../package.json") as { version?: unknown };
      version = typeof manifest.version === "string" ? manifest.version : "unknown";
    } catch {
      version = "unknown";
    }
  }
  return version;
}

/** `initialize`'s result for the TypeScript host and for an answerer that loads nothing
 * (`spec/protocol.md` §2); a {@link Refusal} for a protocol other than this SDK's. The host checks
 * its own params after this. */
export function initializeResult(params: unknown): Record<string, unknown> {
  const protocol = isPlainObject(params) ? params.protocol : undefined;
  if (typeof protocol !== "string") {
    throw new Refusal(INVALID_PARAMS, "initialize needs the engine's protocol");
  }
  if (protocol !== PROTOCOL) {
    throw new Refusal(
      PROTOCOL_MISMATCH,
      `the engine speaks ${protocol} and @grida/fx ${sdkVersion()} speaks ${PROTOCOL}`,
      { engine_protocol: protocol, host_protocol: PROTOCOL },
    );
  }
  return {
    protocol: PROTOCOL,
    host: { language: "typescript", version: process.versions.node ?? "", sdk_version: sdkVersion() },
  };
}

// ------------------------------------------------------------------------------------------------
// Serving on a socket

/**
 * Answers the engine on the stand-in's end of a socket until `exit`, a broken frame or the end of
 * the stream (`spec/protocol.md` §5.7): `initialize` first, then `stand_in.answer` (through
 * `answerer`), `$/cancel`, `shutdown`; any other method is `-32601`. Then it ends the calls still
 * being answered and closes its end. Resolves when it has.
 */
export function serveStandIn(socket: Socket, answerer: Answerer): Promise<void> {
  return new Promise<void>((done) => {
    let initialized = false;
    let ended = false;
    const reader = new FrameReader();
    const end = () => {
      if (ended) {
        return;
      }
      ended = true;
      answerer.close();
      socket.destroy();
      done();
    };
    const send = (message: Record<string, unknown>) => {
      if (ended || socket.destroyed || !socket.writable) {
        return;
      }
      let data: Buffer;
      try {
        data = frame(message);
      } catch (error) {
        // An answer the engine could not read: it still gets one, so it waits for none.
        const why = (error as Error).message;
        const text = encodableText(`the stand-in's answer could not be sent: ${why}`);
        const id = message.id ?? null;
        data = frame({ jsonrpc: "2.0", id, error: { code: INTERNAL, message: text } });
      }
      socket.write(data);
    };
    const refuse = (id: Id | null, code: number, message: string) =>
      send({ jsonrpc: "2.0", id, error: { code, message } });
    const receive = (message: unknown) => {
      if (!isPlainObject(message) || typeof message.method !== "string") {
        return; // this end sends no requests, so it waits for no answer
      }
      const method = message.method;
      const params = message.params;
      if (!Object.hasOwn(message, "id")) {
        if (method === "exit") {
          end();
        } else if (method === "$/cancel" && isPlainObject(params) && validId(params.id)) {
          answerer.cancel(params.id);
        }
        return;
      }
      const id = message.id;
      if (!validId(id)) {
        refuse(null, INVALID_REQUEST, "a request id is an integer or a string");
        return;
      }
      try {
        let result: unknown;
        if (method === "stand_in.answer" && initialized) {
          answerer.start(id, params, (reply) => send({ jsonrpc: "2.0", id, ...reply }));
          return;
        }
        if (method === "initialize") {
          if (initialized) {
            throw new Refusal(INVALID_REQUEST, "initialize was already answered");
          }
          result = initializeResult(params);
          initialized = true;
        } else if (!initialized) {
          throw new Refusal(INVALID_REQUEST, `${method} came before initialize`);
        } else if (method === "shutdown") {
          result = null;
        } else {
          throw new Refusal(METHOD_NOT_FOUND, `@grida/fx's stand-in has no method ${method}`);
        }
        send({ jsonrpc: "2.0", id, result });
      } catch (error) {
        // The answerer's own failure, if it ever fails: the engine still gets an answer.
        const refused =
          error instanceof Refusal
            ? error
            : new Refusal(INTERNAL, `the stand-in's answerer failed: ${errorText(error)}`);
        send({ jsonrpc: "2.0", id, error: refused.body });
      }
    };
    socket.on("data", (chunk: Buffer) => {
      for (const body of reader.push(chunk)) {
        if (ended) {
          return;
        }
        let message: unknown;
        try {
          message = parseMessage(body);
        } catch (error) {
          refuse(null, PARSE_ERROR, encodableText((error as Error).message));
          continue;
        }
        receive(message);
      }
      if (reader.broken !== null) {
        end(); // the stream is broken: closing it tells the engine
      }
    });
    // EPIPE or ECONNRESET as the engine exits is the end of the session too.
    socket.on("error", end);
    socket.on("end", end);
    socket.on("close", end);
  });
}
