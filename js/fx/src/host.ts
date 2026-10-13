/**
 * The TypeScript node host (`spec/protocol.md`), started by the engine as `<node>
 * <@grida/fx>/dist/host-entry.js` with its working directory at the project root. `host-entry.js`
 * has already given the protocol descriptors of their own (descriptor 1 now writes to stderr and
 * descriptor 0 reads the null device), so whatever user code prints, and whatever programs it
 * starts, never reach the protocol. The host reads frames from its protocol input and answers:
 *
 * - `initialize` (§2): first (anything else before it: `-32600`); a protocol other than
 *   `fx-node-protocol-v1` is answered `protocol_mismatch`; `sources` are npm package names; the
 *   result is `{protocol, host: {language: "typescript", version, sdk_version}}`.
 * - `describe` (§5.1): each target module is imported once per session (a failure is kept too),
 *   and reported as `{path, types: [{attribute, spec}], closure}` (every export carrying a node
 *   type's brand, or the one `attribute` names) or `{path, attribute?, error}`. A module whose
 *   closure holds a misspelled file, or that Node loads as CommonJS, fails to import.
 * - `build` (§5.2): in the working directory `cwd`, the builder module's export `function` is
 *   called with one frozen object of the arguments; it returns (or resolves to) the document
 *   `workflow()` makes. `takes_anchor` is the builder's own path.
 * - `run` (§5.3), one at a time: the params are checked against their declared schemas
 *   (`schema.ts`), then the body of the export the body names runs, with a `Ctx` over the
 *   session; its outputs become output values (an `InputFile` by `{file: ref}`, an output under the
 *   work folder by `{work_path, kind}`, any other output stored with `file.put` first; an array as
 *   `{list}`, a `Map` on a keyed port as `{collection}` in its order). The answer waits until the
 *   engine has answered every request of the run; the programs the body left running end once it
 *   is sent. A `NodeFailure` is answered `node_failure` with the body's facts and marks; an engine
 *   error the body let through keeps its code; anything else is `node_error`. A run whose body
 *   loaded a source file outside its module's closure (a computed `import()`) fails the node; what
 *   one run loaded never fails another.
 * - `tool.invoke` and `agent.check` (§5.4, §5.5): an agent of the pending run calls a tool or asks
 *   its check ({@link serveAgents}).
 * - `stand_in.load` and `stand_in.answer` (§5.7): the host answers for a stand-in
 *   ({@link serveStandIns}), and refuses `describe`, `build` and `run` from then on.
 * - `$/cancel` (§5.6): for the pending run, aborts `ctx.signal`; for a request being served
 *   ({@link Host.serve}) or a stand-in's call, answers it `cancelled` at once.
 * - `shutdown` → `null`; then `exit` ends the process with status 0 (1 without `shutdown`), and so
 *   does the end of the input. A pending run is not waited for.
 *
 * Messages name project files by their project-relative paths: the project root is cut from the
 * texts of user errors and their stacks, and this package's own folder is named `@grida/fx`.
 */

import { createReadStream, fstatSync, realpathSync, statSync, writeSync } from "node:fs";
import { Socket } from "node:net";
import { dirname, isAbsolute, normalize, relative, sep } from "node:path";
import type { Readable } from "node:stream";
import { fileURLToPath, pathToFileURL } from "node:url";
import { Worker } from "node:worker_threads";
import { type ClosureEntry, Closures, type Misspelling, NotAnEsModule, type RunScope } from "./closure.js";
import {
  type AgentLoop,
  type Channel,
  type Ctx,
  endAllTools,
  RunContext,
  type RunParams,
  realPath,
  watchTools,
  within,
} from "./ctx.js";
import {
  Cancelled,
  EngineError,
  engineError,
  isEngineError,
  isNodeFailure,
  NodeFailure,
} from "./engine-errors.js";
import { fileRefOf, type Output, outputSourceOf } from "./files.js";
import { isModuleFile } from "./kinds.js";
import {
  type NodeRecord,
  nodeRecordOf,
  PROJECT_PATH,
  type PortShape,
  parsePort,
  SPEC_FIELDS,
  validateTypeSpec,
} from "./node.js";
import {
  BUILD_FAILED,
  CANCELLED,
  checkValue,
  ERROR_CODES,
  FrameReader,
  frame,
  type Id,
  INTERNAL,
  INVALID_PARAMS,
  INVALID_REQUEST,
  isPlainObject,
  LOAD_FAILED,
  METHOD_NOT_FOUND,
  NODE_ERROR,
  NODE_FAILURE,
  PARSE_ERROR,
  type ErrorBody,
  ProtocolError,
  parseMessage,
  pyRepr,
  Refusal,
  safeString,
  typeName,
  validId,
} from "./protocol.js";
import { Answerer, initializeResult, type StandIn } from "./stand-in.js";

/** What a request handler returns when it answers later. */
export const LATER: unique symbol = Symbol("later") as never;

/** How often the host checks that the engine that started it still runs, in milliseconds. */
const ENGINE_CHECK_INTERVAL_MS = 1000;
const LOG_PREFIX = "@grida/fx node host: ";
/** npm package names (`spec/protocol.md` §2: the TypeScript host's `sources`). */
const SOURCE_NAME = /^(?:@[a-z0-9][a-z0-9._~-]*\/)?[a-z0-9][a-z0-9._~-]*$/;
/** A function's or an export's name. */
const IDENTIFIER = /^[A-Za-z_][A-Za-z0-9_]*$/;
/** This package's compiled modules: their frames are cut from a body's stack. */
const SDK_FOLDER = dirname(fileURLToPath(import.meta.url));
/** How texts the host sends name this package's folder (the one holding its `package.json`). */
const OWN_NAME = "@grida/fx";
/** The most programs a body runs at once that the orphan watch ends with the host. */
const WATCHED_TOOLS = 256;

/** Serves one request: its result, {@link LATER} when it answers later, or a promise of either.
 * The session reads its next message only once a returned promise settles, as one reader
 * thread would: answer later instead for work that must not hold the session. A thrown
 * {@link Refusal} is answered with its error. */
export type Handler = (id: Id, params: unknown) => unknown;

/** The pending `run`. */
export interface PendingRun {
  readonly requestId: Id;
  readonly runId: string;
  readonly ctx: RunContext;
  /** Its scope for the closure guard: the body and the host's work for the run run in it. */
  readonly scope: RunScope;
}

/** A module the host loaded: its namespace, its URL and its closure when it first loaded. */
interface Loaded {
  readonly namespace: Record<string, unknown>;
  readonly url: string;
  readonly baseline: ReadonlySet<string>;
}

/** A failure to load a module; its text is the error the engine reports. */
class LoadError extends Error {}

/** Whatever the inbox holds next: a message, a frame that broke the transport, or the end. */
type Incoming =
  | { readonly message: unknown }
  | { readonly unreadable: ProtocolError }
  | { readonly broken: ProtocolError }
  | { readonly end: true };

/** A host request the engine has not answered yet. */
interface Waiting {
  readonly runId: string;
  readonly method: string;
  readonly resolve: (value: unknown) => void;
  readonly reject: (error: unknown) => void;
  /** Settles when the answer arrives (or the session ends), whatever the caller does: a run is
   * answered only after every one of these. */
  readonly answered: Promise<void>;
  readonly settle: () => void;
}

/** The host's protocol stream: whole frames written at once, and its own numbered requests. */
export class Session {
  readonly #output: number;
  readonly #fail: (error: unknown) => never;
  #nextId = 1;
  readonly #waiting = new Map<Id, Waiting>();
  #closed = false;

  constructor(output: number, fail: (error: unknown) => never) {
    this.#output = output;
    this.#fail = fail;
  }

  /** Writes one message as one frame. A `TypeError` (nothing written) when it is not an I-JSON
   * value; a broken stream ends the host. */
  send(message: unknown): void {
    let data = frame(message);
    while (data.length > 0) {
      try {
        data = data.subarray(writeSync(this.#output, data));
      } catch (error) {
        if ((error as { code?: unknown }).code === "EAGAIN") {
          continue;
        }
        this.#fail(error);
      }
    }
  }

  /** Sends the host request `method` for `runId` and waits for its result. A `TypeError` (nothing
   * sent) when the params are not an I-JSON value the engine can read. */
  request(runId: string, method: string, params: Record<string, unknown>): Promise<unknown> {
    const sent = { run_id: runId, ...params };
    try {
      checkValue(sent, "", 1);
    } catch (error) {
      return Promise.reject(new TypeError(`${method}: ${(error as Error).message}`));
    }
    if (this.#closed) {
      return Promise.reject(new Cancelled("the engine ended the session"));
    }
    const id = this.#nextId;
    this.#nextId += 1;
    let settle: () => void = () => {};
    const answered = new Promise<void>((done) => {
      settle = done;
    });
    const result = new Promise<unknown>((resolve, reject) => {
      this.#waiting.set(id, { runId, method, resolve, reject, answered, settle });
    });
    try {
      this.send({ jsonrpc: "2.0", id, method, params: sent });
    } catch (error) {
      this.#waiting.delete(id);
      settle();
      return Promise.reject(error);
    }
    return result;
  }

  /** Sends a notification for `runId`. A `TypeError` (nothing sent) for params the engine could
   * not read. */
  notify(runId: string, method: string, params: Record<string, unknown>): void {
    const sent = { run_id: runId, ...params };
    try {
      checkValue(sent, "", 1);
    } catch (error) {
      throw new TypeError(`${method}: ${(error as Error).message}`);
    }
    this.send({ jsonrpc: "2.0", method, params: sent });
  }

  /** Resolves the host request a response answers; false when none is pending. */
  deliver(message: Record<string, unknown>): boolean {
    const id = message.id;
    if (typeof id !== "number" && typeof id !== "string") {
      return false;
    }
    const waiting = this.#waiting.get(id);
    if (waiting === undefined) {
      return false;
    }
    this.#waiting.delete(id);
    if (Object.hasOwn(message, "error")) {
      waiting.reject(answerError(waiting.method, message.error));
    } else if (Object.hasOwn(message, "result")) {
      waiting.resolve(message.result);
    } else {
      const text = `the engine answered ${waiting.method} with neither a result nor an error`;
      waiting.reject(new EngineError(text, null, INTERNAL));
    }
    waiting.settle();
    return true;
  }

  /** Waits until the engine has answered every host request of `runId` (§5.3: a run is not
   * answered while one of its own requests is pending). */
  async settle(runId: string): Promise<void> {
    for (;;) {
      const waiting = [...this.#waiting.values()]
        .filter((entry) => entry.runId === runId)
        .map((entry) => entry.answered);
      if (waiting.length === 0) {
        return;
      }
      await Promise.all(waiting);
    }
  }

  /** Ends the session: every pending host request fails with `Cancelled`, and later ones at
   * once. */
  close(): void {
    this.#closed = true;
    const waiting = [...this.#waiting.values()];
    this.#waiting.clear();
    for (const entry of waiting) {
      entry.reject(new Cancelled("the engine ended the session"));
      entry.settle();
    }
  }

  /** The requests of one run, for its context. */
  channel(runId: string): Channel {
    return {
      request: (method, params) => this.request(runId, method, params),
      notify: (method, params) => this.notify(runId, method, params),
    };
  }
}

function answerError(method: string, error: unknown): EngineError {
  if (isPlainObject(error)) {
    const { code, message } = error;
    if (typeof code === "number" && Number.isInteger(code) && typeof message === "string") {
      return engineError(code, message, Object.hasOwn(error, "data") ? error.data : null);
    }
  }
  return new EngineError(`the engine answered ${method} with a malformed error`, null, INTERNAL);
}

/** One host session (module comment). */
export class Host {
  readonly session: Session;
  readonly closures = new Closures();
  initialized = false;
  shuttingDown = false;
  /** The project root, resolved. */
  root = "";
  sources: string[] = [];
  /**
   * Methods served for the pending run, routed before the shutdown check (a pending run still
   * finishes after `shutdown`): `tool.invoke`, `agent.check`.
   */
  readonly runMethods = new Map<string, Handler>();
  /** Methods served after the shutdown check and before `describe`, `build` and `run`:
   * `stand_in.load`, `stand_in.answer`. */
  readonly methods = new Map<string, Handler>();
  /** Asked, in order, about a `$/cancel` for an id this host does not otherwise know; the first
   * that returns true handled it. */
  readonly cancellers: ((id: Id) => boolean)[] = [];
  /** Set once this host answers for a stand-in: `describe`, `build` and `run` are then refused. */
  standingIn = false;
  readonly #exit: (status: number) => never;
  readonly #inbox: Incoming[] = [];
  #pumping = false;
  #ended = false;
  #rootTexts: string[] = [];
  /** What {@link #unrooted} cuts, longest first: each text and what it becomes. */
  #cuts: (readonly [string, string])[] = ownCuts();
  #run: PendingRun | null = null;
  /** Requests served by {@link serve}: id → method, and how to abort its work. */
  readonly #serving = new Map<Id, { readonly method: string; readonly controller: AbortController }>();
  readonly #modules = new Map<string, Loaded>();
  /** Modules that failed to load, by project-relative path: each is loaded at most once. */
  readonly #failures = new Map<string, string>();

  constructor(output: number, exit: (status: number) => never) {
    this.#exit = exit;
    this.session = new Session(output, (error) => {
      log(`could not write to the engine: ${String((error as Error)?.message ?? error)}`);
      return this.#end(1);
    });
  }

  /** The pending run, if one is. */
  pendingRun(): PendingRun | null {
    return this.#run;
  }

  // -- the session -------------------------------------------------------------------------

  /** Reads frames from `input` until `exit`, a broken frame or the end of the input. */
  listen(input: number): void {
    const reader = new FrameReader();
    const stream = protocolInput(input);
    stream.on("data", (chunk: Buffer | string) => {
      for (const body of reader.push(typeof chunk === "string" ? Buffer.from(chunk) : chunk)) {
        try {
          this.#receive({ message: parseMessage(body) });
        } catch (error) {
          this.#receive({ unreadable: error as ProtocolError });
        }
      }
      if (reader.broken !== null) {
        stream.destroy();
        this.#receive({ broken: reader.broken });
      }
    });
    stream.on("end", () => {
      try {
        reader.end();
        this.#receive({ end: true });
      } catch (error) {
        this.#receive({ broken: error as ProtocolError });
      }
    });
    stream.on("error", (error) => {
      log(`could not read from the engine: ${error.message}`);
      this.#receive({ end: true });
    });
  }

  #receive(incoming: Incoming): void {
    this.#inbox.push(incoming);
    void this.#pump();
  }

  /** Handles messages in order; a request whose handler returns a promise holds the next one until
   * it settles (a describe, a build, a module loading for a run). */
  async #pump(): Promise<void> {
    if (this.#pumping || this.#ended) {
      return;
    }
    this.#pumping = true;
    try {
      while (this.#inbox.length > 0 && !this.#ended) {
        const incoming = this.#inbox.shift() as Incoming;
        if ("end" in incoming) {
          this.#end(this.shuttingDown ? 0 : 1);
        } else if ("broken" in incoming) {
          log(incoming.broken.message);
          this.#sendError(null, PARSE_ERROR, incoming.broken.message);
          this.#end(1);
        } else if ("unreadable" in incoming) {
          this.#sendError(null, PARSE_ERROR, incoming.unreadable.message);
        } else {
          const handled = this.#dispatch(incoming.message);
          if (handled !== undefined) {
            await handled;
          }
        }
      }
    } finally {
      this.#pumping = false;
    }
  }

  #end(status: number): never {
    this.#ended = true;
    this.session.close();
    endAllTools();
    return this.#exit(status);
  }

  /** Handles one message; a promise when the next must wait for it. */
  #dispatch(message: unknown): Promise<void> | undefined {
    if (!isPlainObject(message)) {
      this.#sendError(null, INVALID_REQUEST, "a message is a JSON object");
      return undefined;
    }
    const requestId = message.id;
    const knownId = validId(requestId) ? requestId : null;
    if (message.jsonrpc !== "2.0") {
      this.#sendError(knownId, INVALID_REQUEST, 'a message carries "jsonrpc": "2.0"');
      return undefined;
    }
    if (!Object.hasOwn(message, "method")) {
      // A response to one of this host's requests (a run's).
      if (!this.session.deliver(message)) {
        log(`ignored a response to id ${pyRepr(requestId)}`);
      }
      return undefined;
    }
    const method = message.method;
    if (typeof method !== "string") {
      this.#sendError(knownId, INVALID_REQUEST, "a message's method is a string");
      return undefined;
    }
    if (!Object.hasOwn(message, "id")) {
      this.#notify(method, message.params);
      return undefined;
    }
    if (knownId === null) {
      this.#sendError(null, INVALID_REQUEST, "a request id is an integer or a string");
      return undefined;
    }
    let outcome: unknown;
    try {
      outcome = this.#request(method, message.params, knownId);
    } catch (error) {
      this.#refused(knownId, error);
      return undefined;
    }
    if (outcome instanceof Promise) {
      return outcome.then(
        (result) => {
          if (result !== LATER) {
            this.#sendResult(knownId, result);
          }
        },
        (error: unknown) => this.#refused(knownId, error),
      );
    }
    if (outcome !== LATER) {
      this.#sendResult(knownId, outcome);
    }
    return undefined;
  }

  #refused(id: Id, error: unknown): void {
    if (error instanceof Refusal) {
      this.session.send({ jsonrpc: "2.0", id, error: error.body });
      return;
    }
    log(this.#unrooted(stackOf(error)));
    this.#sendError(id, INTERNAL, `the node host failed: ${this.errorText(error)}`);
  }

  #notify(method: string, params: unknown): void {
    if (method === "exit") {
      this.#end(this.shuttingDown ? 0 : 1);
    }
    if (method === "$/cancel") {
      this.#cancel(params);
    }
    // Other notifications are ignored.
  }

  #request(method: string, params: unknown, id: Id): unknown {
    if (method === "initialize") {
      if (this.initialized) {
        throw new Refusal(INVALID_REQUEST, "initialize was already answered");
      }
      return this.#initialize(objectParams(params));
    }
    if (!this.initialized) {
      throw new Refusal(INVALID_REQUEST, `${method} came before initialize`);
    }
    // Part of the pending run, which shutdown lets finish.
    const forRun = this.runMethods.get(method);
    if (forRun !== undefined) {
      return forRun(id, params);
    }
    if (this.shuttingDown) {
      throw new Refusal(INVALID_REQUEST, `${method} came after shutdown`);
    }
    if (method === "shutdown") {
      if (params !== undefined && params !== null) {
        throw new Refusal(INVALID_PARAMS, "shutdown takes no params");
      }
      this.shuttingDown = true;
      return null;
    }
    const other = this.methods.get(method);
    if (other !== undefined) {
      return other(id, params);
    }
    const job = method === "describe" || method === "build" || method === "run";
    if (job && this.standingIn) {
      throw new Refusal(INVALID_REQUEST, "this host answers for a stand-in");
    }
    if (job && this.#run !== null) {
      throw new Refusal(INVALID_REQUEST, `${method} came while a run is pending`);
    }
    if (method === "describe") {
      return this.#describe(objectParams(params));
    }
    if (method === "build") {
      return this.#build(objectParams(params));
    }
    if (method === "run") {
      return this.#startRun(id, objectParams(params));
    }
    throw new Refusal(METHOD_NOT_FOUND, `the TypeScript node host has no method ${method}`);
  }

  #sendResult(id: Id, result: unknown): void {
    try {
      this.session.send({ jsonrpc: "2.0", id, result });
    } catch (error) {
      const why = (error as Error).message;
      this.#sendError(id, INTERNAL, `the node host's answer is not an I-JSON value: ${why}`);
    }
  }

  #sendError(id: Id | null, code: number, message: string, data?: unknown): void {
    this.session.send({ jsonrpc: "2.0", id, error: new Refusal(code, message, data).body });
  }

  /** Answers a request served later: a result, or an error (whose `data` is left out when it is
   * not an I-JSON value). */
  answer(id: Id, outcome: { readonly result: unknown } | { readonly error: ErrorBody }): void {
    try {
      if ("result" in outcome) {
        this.#sendResult(id, outcome.result);
        return;
      }
      const { code, message, data } = outcome.error;
      try {
        this.#sendError(id, code, message, data);
      } catch {
        // data that is not an I-JSON value (a body changed ctx.facts by hand): left out.
        this.#sendError(id, code, message);
      }
    } catch (unsent) {
      // The last resort: an answer the engine can read, so it waits for none.
      this.#sendError(id, INTERNAL, `the answer could not be sent: ${(unsent as Error).message}`);
    }
  }

  /**
   * Serves a request whose answer `work` makes (`tool.invoke`, `agent.check`): a `$/cancel` for it
   * answers it `cancelled` at once and aborts the signal `work` was given, and what `work` makes
   * after that is dropped. A `NodeFailure` is answered `node_failure`; anything else `work` throws
   * is a `node_error`.
   */
  serve(id: Id, method: string, work: (signal: AbortSignal) => Promise<unknown>): typeof LATER {
    const controller = new AbortController();
    this.#serving.set(id, { method, controller });
    const run = this.#run;
    void (async () => {
      let outcome: { result: unknown } | { error: ErrorBody };
      try {
        // Part of the pending run: what it loads is the run's (Closures.within).
        const working = () => work(controller.signal);
        outcome = { result: await (run === null ? working() : this.closures.within(run.scope, working)) };
      } catch (error) {
        outcome = {
          error: isNodeFailure(error)
            ? { code: NODE_FAILURE, message: this.#message(error) }
            : this.nodeError(error),
        };
      }
      if (this.#serving.get(id)?.controller !== controller) {
        return; // answered `cancelled` already
      }
      this.#serving.delete(id);
      this.answer(id, outcome);
    })();
    return LATER;
  }

  #cancel(params: unknown): void {
    if (!isPlainObject(params) || !validId(params.id)) {
      return;
    }
    const target = params.id;
    const run = this.#run;
    if (run !== null && run.requestId === target) {
      // What the body's abort listeners load is the run's.
      this.closures.within(run.scope, () => run.ctx.cancel(new Error("the engine cancelled run")));
      return;
    }
    const served = this.#serving.get(target);
    if (served !== undefined) {
      this.#serving.delete(target);
      const message = `the engine cancelled ${served.method}`;
      this.answer(target, { error: { code: CANCELLED, message } });
      served.controller.abort(new Error(message));
      return;
    }
    for (const canceller of this.cancellers) {
      if (canceller(target)) {
        return;
      }
    }
  }

  // -- initialize --------------------------------------------------------------------------

  #initialize(params: Record<string, unknown>): unknown {
    const result = initializeResult(params);
    const given = params.project_root;
    if (typeof given !== "string" || !given || !isAbsolute(given)) {
      throw new Refusal(INVALID_PARAMS, "project_root is the project's absolute path");
    }
    const root = realPath(given);
    let folder = false;
    try {
      folder = statSync(root).isDirectory();
    } catch {
      folder = false;
    }
    if (!folder) {
      throw new Refusal(INVALID_PARAMS, "project_root is not a folder");
    }
    const sources = params.sources;
    const named = (name: unknown) => typeof name === "string" && SOURCE_NAME.test(name);
    if (!Array.isArray(sources) || !sources.every(named)) {
      throw new Refusal(INVALID_PARAMS, "sources is a list of package names");
    }
    this.root = root;
    this.sources = [...(sources as string[])];
    const trimmed = given.replace(/\/+$/, "") || given;
    this.#rootTexts = [...new Set([trimmed, root])].sort((a, b) => b.length - a.length);
    const rootCuts = this.#rootTexts.flatMap((text) => [
      [`${pathToFileURL(text).href}/`, ""] as const,
      [`${text}${sep}`, ""] as const,
    ]);
    this.#cuts = [...rootCuts, ...ownCuts()].sort((a, b) => b[0].length - a[0].length);
    this.closures.configure(root, this.sources);
    this.initialized = true;
    return result;
  }

  // -- describe ----------------------------------------------------------------------------

  async #describe(params: Record<string, unknown>): Promise<unknown> {
    const targets = params.targets;
    if (!Array.isArray(targets)) {
      throw new Refusal(INVALID_PARAMS, "describe needs targets: a list");
    }
    if (typeof params.builtins !== "boolean") {
      throw new Refusal(INVALID_PARAMS, "describe needs builtins: true or false");
    }
    const checked: [string, string | null][] = [];
    for (const target of targets) {
      if (!isPlainObject(target)) {
        throw new Refusal(INVALID_PARAMS, "a describe target is {path, attribute?}");
      }
      const path = projectPath(target.path, "a describe target's path");
      const attribute = target.attribute;
      const given = attribute !== undefined && attribute !== null;
      if (given && (typeof attribute !== "string" || !attribute)) {
        throw new Refusal(INVALID_PARAMS, "a describe target's attribute is a name");
      }
      checked.push([path, typeof attribute === "string" ? attribute : null]);
    }
    const modules: unknown[] = [];
    for (const [path, attribute] of checked) {
      modules.push(await this.#describeTarget(path, attribute));
    }
    // The engine declares every built-in; a TypeScript host carries none.
    return { modules, builtins: [] };
  }

  async #describeTarget(path: string, attribute: string | null): Promise<unknown> {
    const failed = (error: string) => ({ path, ...(attribute !== null ? { attribute } : {}), error });
    let loaded: Loaded;
    try {
      loaded = await this.#load(path, `no module file ${path}`);
    } catch (error) {
      if (error instanceof LoadError) {
        return failed(error.message);
      }
      throw error;
    }
    const declared: [string, NodeRecord][] = [];
    if (attribute !== null) {
      const record = nodeRecordOf(exportOf(loaded.namespace, attribute));
      if (record === undefined) {
        return failed(`${path}: ${attribute} is not an export declared with node()`);
      }
      declared.push([attribute, record]);
    } else {
      for (const name of Object.keys(loaded.namespace)) {
        if (name === "default" || !IDENTIFIER.test(name)) {
          continue;
        }
        const record = nodeRecordOf(exportOf(loaded.namespace, name));
        if (record !== undefined) {
          declared.push([name, record]);
        }
      }
    }
    const types: unknown[] = [];
    for (const [name, record] of declared) {
      try {
        // The protocol's fields in its order, whichever copy of @grida/fx made the spec.
        const given = record.spec as unknown as Record<string, unknown>;
        const spec: Record<string, unknown> = { name: given.name };
        if (given.description !== undefined) {
          spec.description = given.description;
        }
        for (const field of SPEC_FIELDS) {
          spec[field] = given[field];
        }
        validateTypeSpec(spec);
        checkValue(spec);
        types.push({ attribute: name, spec });
      } catch (error) {
        return failed(`${path}: ${name}: ${(error as Error).message}`);
      }
    }
    const closure = this.closures.closure(loaded.url).map((entry: ClosureEntry) => ({
      label: entry.label,
      path: entry.path,
    }));
    return { path, types, closure };
  }

  // -- build -------------------------------------------------------------------------------

  async #build(params: Record<string, unknown>): Promise<unknown> {
    const path = projectPath(params.path, "build's path");
    const fn = params.function;
    if (typeof fn !== "string" || !IDENTIFIER.test(fn)) {
      throw new Refusal(INVALID_PARAMS, "build's function is a name");
    }
    const args = params.arguments;
    if (!isPlainObject(args) || !Object.values(args).every((value) => typeof value === "string")) {
      throw new Refusal(INVALID_PARAMS, "build's arguments map names to strings");
    }
    const cwd = params.cwd;
    if (typeof cwd !== "string" || !cwd || !isAbsolute(cwd)) {
      throw new Refusal(INVALID_PARAMS, "build's cwd is an absolute path");
    }
    const previous = process.cwd();
    try {
      process.chdir(cwd);
    } catch {
      throw new Refusal(INVALID_PARAMS, "build's cwd is not a folder this host can enter");
    }
    // The builder module's top-level code and the builder both run in cwd (§5.2).
    let document: unknown;
    try {
      document = await this.#buildInCwd(path, fn, args as Record<string, string>);
    } finally {
      try {
        process.chdir(previous);
      } catch {
        process.chdir(this.root);
      }
    }
    return { document, takes_anchor: path };
  }

  async #buildInCwd(path: string, fn: string, args: Record<string, string>): Promise<unknown> {
    let loaded: Loaded;
    try {
      loaded = await this.#load(path, `no builder file ${path}`);
    } catch (error) {
      if (error instanceof LoadError) {
        throw new Refusal(LOAD_FAILED, error.message);
      }
      throw error;
    }
    const builder = exportOf(loaded.namespace, fn);
    if (typeof builder !== "function") {
      throw new Refusal(LOAD_FAILED, `${path} has no function ${fn}`);
    }
    let built: unknown;
    try {
      const call = builder as (args: Readonly<Record<string, string>>) => unknown;
      built = await call(Object.freeze({ ...args }));
    } catch (error) {
      throw new Refusal(BUILD_FAILED, `${fn}: ${this.errorText(error)}`);
    }
    if (!isPlainObject(built) || built.fx !== "workflow/v1") {
      throw new Refusal(BUILD_FAILED, `${fn} returned ${typeName(built)}, not a workflow document`);
    }
    try {
      checkValue(built);
    } catch (error) {
      throw new Refusal(BUILD_FAILED, `${fn}: ${this.errorText(error)}`);
    }
    return built;
  }

  // -- run ---------------------------------------------------------------------------------

  /** Loads the body a `run` names, then runs it; the run is answered when the body is done. */
  async #startRun(id: Id, params: Record<string, unknown>): Promise<typeof LATER> {
    checkRun(params);
    const target = await this.#body(params.body as Record<string, unknown>);
    if (this.#run !== null) {
      throw new Refusal(INVALID_REQUEST, "run came while a run is pending");
    }
    const run = params as unknown as RunParams;
    const state: PendingRun = {
      requestId: id,
      runId: run.run_id,
      ctx: new RunContext(run, this.session.channel(run.run_id), (text) => this.#unrooted(text)),
      scope: this.closures.openRun(),
    };
    this.#run = state;
    void this.#execute(state, target);
    return LATER;
  }

  async #body(body: Record<string, unknown>): Promise<{
    readonly record: NodeRecord;
    readonly name: string;
    readonly loaded: Loaded;
    readonly outputs: Readonly<Record<string, PortShape>>;
  }> {
    if (Object.hasOwn(body, "builtin")) {
      const builtin = body.builtin;
      if (typeof builtin !== "string" || Object.keys(body).length !== 1) {
        throw new Refusal(INVALID_PARAMS, "run's body is {path, attribute} or {builtin}");
      }
      throw new Refusal(LOAD_FAILED, `@grida/fx has no body for ${builtin}`);
    }
    const path = projectPath(body.path, "run's body path");
    const attribute = body.attribute;
    if (typeof attribute !== "string" || !attribute) {
      throw new Refusal(INVALID_PARAMS, "run's body attribute is a name");
    }
    let loaded: Loaded;
    try {
      loaded = await this.#load(path, `no module file ${path}`);
    } catch (error) {
      if (error instanceof LoadError) {
        throw new Refusal(LOAD_FAILED, error.message);
      }
      throw error;
    }
    const record = nodeRecordOf(exportOf(loaded.namespace, attribute));
    if (record === undefined) {
      throw new Refusal(LOAD_FAILED, `${path}: ${attribute} is not an export declared with node()`);
    }
    const outputs: Record<string, PortShape> = {};
    const declared = (record.spec as { outputs?: unknown }).outputs;
    if (isPlainObject(declared)) {
      for (const [port, notation] of Object.entries(declared)) {
        try {
          outputs[port] = parsePort(notation);
        } catch {
          // left out: the engine checks the spec it planned with
        }
      }
    }
    return { record, name: String(record.spec.name), loaded, outputs };
  }

  async #execute(
    state: PendingRun,
    target: {
      readonly record: NodeRecord;
      readonly name: string;
      readonly loaded: Loaded;
      readonly outputs: Readonly<Record<string, PortShape>>;
    },
  ): Promise<void> {
    const { ctx } = state;
    let outcome: { result: unknown } | { error: ErrorBody };
    try {
      ctx.checkParams((target.record.spec as { params?: Record<string, unknown> }).params ?? {});
      let returned: unknown;
      let raised: { readonly error: unknown } | null = null;
      try {
        // Called as its record's method: a traceback names the frame `Object.body`.
        returned = await this.closures.within(state.scope, () =>
          (target.record.body as (ctx: Ctx) => unknown)(ctx as unknown as Ctx),
        );
      } catch (error) {
        raised = { error };
      }
      const late = this.#loadedLate(state.scope, target.loaded);
      if (late !== null) {
        throw new NodeFailure(late);
      }
      if (raised !== null) {
        throw raised.error;
      }
      const outputs = await this.#outputs(ctx, target.name, target.outputs, returned);
      outcome = { result: { outputs } };
    } catch (error) {
      outcome = { error: this.#failure(error, ctx) };
    }
    await this.session.settle(state.runId);
    if (this.#run === state) {
      this.#run = null;
    }
    this.answer(state.requestId, outcome);
    // What the body started and left running ends with its run.
    ctx.endTools();
  }

  /** Why the run fails for the first source file the body loaded at run time outside the
   * module's closure ({@link Closures.closeRun}), or null: `the body loaded <label> at run time,
   * outside its source closure: import it statically`, or, for a file spelled unlike its directory
   * entries, which no import names (§5.1), `…: <label> is spelled <on disk> on disk`. The run's
   * own edges are then dropped: a later run fails only if it loads such a file itself. */
  #loadedLate(scope: RunScope, loaded: Loaded): string | null {
    const late = this.closures.closeRun(scope, loaded.url, loaded.baseline)[0];
    if (late === undefined) {
      return null;
    }
    const misspelled = this.closures.misspelling([late]);
    const advice = misspelled === null ? "import it statically" : spelledOnDisk(misspelled);
    return `the body loaded ${late.label} at run time, outside its source closure: ${advice}`;
  }

  /** A body's outputs as output values (§3.4). */
  async #outputs(
    ctx: RunContext,
    name: string,
    shapes: Readonly<Record<string, PortShape>>,
    returned: unknown,
  ): Promise<Record<string, unknown>> {
    if (returned === undefined || returned === null) {
      return {};
    }
    if (!isPlainObject(returned)) {
      throw new NodeFailure(`${name} returned ${typeName(returned)}, not its outputs`);
    }
    const outputs: Record<string, unknown> = {};
    for (const [port, value] of Object.entries(returned)) {
      const shape = Object.hasOwn(shapes, port) ? (shapes[port] as PortShape).shape : null;
      outputs[port] = await this.#portOutput(ctx, port, shape, value);
    }
    return outputs;
  }

  async #portOutput(
    ctx: RunContext,
    port: string,
    shape: string | null,
    value: unknown,
  ): Promise<unknown> {
    if (value === undefined || value === null) {
      return null;
    }
    if (Array.isArray(value)) {
      const list: unknown[] = [];
      for (let index = 0; index < value.length; index += 1) {
        list.push(await this.#outputValue(ctx, value[index], `${port}[${index}]`));
      }
      return { list };
    }
    if (value instanceof Map && (shape === "keyed" || shape === null)) {
      const collection: unknown[] = [];
      for (const [key, item] of value) {
        if (typeof key !== "string") {
          throw new NodeFailure(`output ${port} has the key ${pyRepr(key)}, which is not a string`);
        }
        collection.push([key, await this.#outputValue(ctx, item, `${port}[${key}]`)]);
      }
      return { collection };
    }
    if (isPlainObject(value) && shape === "keyed") {
      throw new NodeFailure(`output ${port} is an object; a keyed output is a Map`);
    }
    return this.#outputValue(ctx, value, port);
  }

  async #outputValue(ctx: RunContext, item: unknown, label: string): Promise<unknown> {
    const ref = fileRefOf(item);
    if (ref !== undefined) {
      return { file: ref };
    }
    const source = outputSourceOf(item);
    if (source !== undefined) {
      if ("workPath" in source) {
        return { work_path: source.workPath, kind: (item as Output).kind };
      }
      return { file: await ctx.put(item as Output, `${ctx.instance.path}/${label}`) };
    }
    throw new NodeFailure(`output ${label} is ${typeName(item)}; use ctx.out to make it`);
  }

  /** How a run that did not return its outputs is answered. */
  #failure(error: unknown, ctx: RunContext): ErrorBody {
    if (isNodeFailure(error)) {
      return { code: NODE_FAILURE, message: this.#message(error), data: ctx.kept() };
    }
    if (isEngineError(error)) {
      if (ERROR_CODES.has(error.code)) {
        const message = typeof error.message === "string" && error.message ? error.message : error.name;
        return isPlainObject(error.data)
          ? { code: error.code, message, data: error.data }
          : { code: error.code, message };
      }
    }
    return this.nodeError(error, ctx);
  }

  #message(error: unknown): string {
    const message = this.#unrooted(String((error as Error).message ?? ""));
    return message || String((error as Error).name || "Error");
  }

  /** A `node_error`: the error's own message, with its name and stack (and the body's facts and
   * marks) in `data`. The engine names the failure `<exception>: <message>`, so the message does
   * not repeat the name; an error with no message sends its name, since a message is never
   * empty (§7). */
  nodeError(error: unknown, ctx?: RunContext): ErrorBody {
    let exception = "NonError";
    let message: string;
    let traceback = "";
    if (isErrorLike(error)) {
      exception = String(error.name || "Error");
      message = this.#unrooted(String(error.message ?? ""));
      traceback = this.#traceback(error);
    } else {
      message = this.#unrooted(safeString(error));
    }
    const data: Record<string, unknown> = { exception, traceback, ...(ctx?.kept() ?? {}) };
    return { code: NODE_ERROR, message: message || exception, data };
  }

  /** An error's stack from the first frame of user code: the host's frames and Node's own are cut,
   * and so is the project root. */
  #traceback(error: Error): string {
    const stack = typeof error.stack === "string" ? error.stack : `${error.name}: ${error.message}`;
    const sdk = [`${SDK_FOLDER}${sep}`, `${pathToFileURL(SDK_FOLDER).href}/`, "node:internal"];
    const kept = stack
      .split("\n")
      .filter((line) => !/^\s+at /.test(line) || !sdk.some((part) => line.includes(part)));
    return this.#unrooted(kept.join("\n"));
  }

  // -- modules -----------------------------------------------------------------------------

  /** The project module at `path`, imported once per session (a failure is kept too). */
  async #load(path: string, missing: string): Promise<Loaded> {
    const file = realPath(`${this.root}${sep}${path}`);
    if (!within(file, this.root)) {
      throw new LoadError(`${path} is outside the project`);
    }
    const relativePath = relative(this.root, file).split(sep).join("/");
    const known = this.#modules.get(relativePath);
    if (known !== undefined) {
      return known;
    }
    const failure = this.#failures.get(relativePath);
    if (failure !== undefined) {
      throw new LoadError(failure);
    }
    let isFile = false;
    try {
      isFile = statSync(file).isFile();
    } catch {
      isFile = false;
    }
    if (!isFile) {
      throw new LoadError(missing);
    }
    if (!isModuleFile(file)) {
      throw new LoadError(`cannot import ${relativePath}`);
    }
    const url = pathToFileURL(file).href;
    let text: string;
    try {
      const namespace = await this.closures.importModule(url);
      const closure = this.closures.closure(url);
      const misspelled = this.closures.misspelling(closure);
      if (misspelled === null) {
        const baseline = new Set(closure.map((entry) => entry.path));
        const loaded: Loaded = { namespace, url, baseline };
        this.#modules.set(relativePath, loaded);
        return loaded;
      }
      text = `${relativePath} failed to import: ${spelledOnDisk(misspelled)}`;
    } catch (error) {
      text = this.importFailure(relativePath, error);
    }
    this.#failures.set(relativePath, text);
    throw new LoadError(text);
  }

  /** `<shown> failed to import: <why>` (§5.1), for the module the host named `shown`. */
  importFailure(shown: string, error: unknown): string {
    const why =
      error instanceof NotAnEsModule
        ? `${shown} is not an ES module: set "type": "module" in package.json`
        : this.errorText(error);
    return `${shown} failed to import: ${why}`;
  }

  /**
   * An error as a sentence: `<code>: <message>` when it has a Node error code (`ERR_…`), else
   * `<name>: <message>`, or the name alone when the message is empty; a thrown value that is not
   * an error is `NonError: <value>`. A syntax error gets `(<file>, line <n>)` when its stack
   * names a project file. The project root is cut from the text.
   */
  errorText(error: unknown): string {
    if (!isErrorLike(error)) {
      const text = this.#unrooted(safeString(error));
      return text ? `NonError: ${text}` : "NonError";
    }
    const code = (error as { code?: unknown }).code;
    const head =
      typeof code === "string" && code.startsWith("ERR_") ? code : String(error.name || "Error");
    let message = this.#unrooted(String(error.message ?? ""));
    if (error.name === "SyntaxError") {
      const where = this.#syntaxLocation(typeof error.stack === "string" ? error.stack : "");
      if (where !== null && !message.includes(where)) {
        message = `${message} (${where})`;
      }
    }
    return message ? `${head}: ${message}` : head;
  }

  /** `<label>, line <n>` of the first project file a syntax error's stack names. */
  #syntaxLocation(stack: string): string | null {
    for (const root of this.#rootTexts) {
      for (const prefix of [`${pathToFileURL(root).href}/`, `${root}${sep}`]) {
        const at = stack.indexOf(prefix);
        if (at < 0) {
          continue;
        }
        const match = /^([^\s:]+):(\d+)/.exec(stack.slice(at + prefix.length));
        if (match !== null) {
          return `${decodeURIComponent(match[1] as string)}, line ${match[2]}`;
        }
      }
    }
    return null;
  }

  /** `text` with every path under the project root made project-relative, and this package's
   * folder named `@grida/fx`. */
  #unrooted(text: string): string {
    let cut = text;
    for (const [found, replaced] of this.#cuts) {
      cut = cut.split(found).join(replaced);
    }
    return cut;
  }

  /** {@link #unrooted}, for other modules of the host. */
  unrooted(text: string): string {
    return this.#unrooted(text);
  }
}

/** A request's params: a `-32602` refusal unless they are a JSON object. */
export function objectParams(params: unknown): Record<string, unknown> {
  if (!isPlainObject(params)) {
    throw new Refusal(INVALID_PARAMS, "this method's params are a JSON object");
  }
  return params;
}

function projectPath(value: unknown, what: string): string {
  if (typeof value !== "string" || !PROJECT_PATH.test(value)) {
    throw new Refusal(
      INVALID_PARAMS,
      `${what} is a POSIX path relative to the project root, not ${pyRepr(value)}`,
    );
  }
  return value;
}

/** Refuses `run` params that do not have the shape of §5.3. */
function checkRun(params: Record<string, unknown>): void {
  const runId = params.run_id;
  if (typeof runId !== "string" || !runId) {
    throw new Refusal(INVALID_PARAMS, "run's run_id is a non-empty string");
  }
  const instance = params.instance;
  const take = isPlainObject(instance) ? instance.take : undefined;
  if (
    !isPlainObject(instance) ||
    !["id", "path", "step"].every((field) => typeof instance[field] === "string") ||
    !(instance.key === undefined || instance.key === null || typeof instance.key === "string") ||
    !Array.isArray(take) ||
    take.length === 0 ||
    !take.every((number) => typeof number === "number" && Number.isInteger(number))
  ) {
    throw new Refusal(INVALID_PARAMS, "run's instance is {id, path, step, key, take}");
  }
  if (!isPlainObject(params.body)) {
    throw new Refusal(INVALID_PARAMS, "run's body is {path, attribute} or {builtin}");
  }
  for (const field of ["params", "param_files", "inputs", "resources", "tools", "calls"]) {
    if (!isPlainObject(params[field])) {
      throw new Refusal(INVALID_PARAMS, `run's ${field} is a JSON object`);
    }
  }
  const workDir = params.work_dir;
  if (typeof workDir !== "string" || !workDir || !isAbsolute(workDir)) {
    throw new Refusal(INVALID_PARAMS, "run's work_dir is an absolute path");
  }
  const timeout = params.timeout_s;
  if (!(timeout === undefined || timeout === null || typeof timeout === "number")) {
    throw new Refusal(INVALID_PARAMS, "run's timeout_s is a number of seconds or null");
  }
}

/**
 * The protocol input as a stream. A pipe or a socket (the engine gives a pipe) is read through
 * libuv's own handle, which waits for input in the event loop, so the host can end at any time,
 * also while the engine keeps the input open. A file system read waits in a thread of libuv's
 * pool, and Node's exit waits for that thread: a host that ended itself (a write to the engine
 * that failed) would live on until the engine closed the input. Anything else is read through the
 * file system: a file, whose reads do not wait, and a named pipe on macOS, whose end of input
 * kqueue never reports.
 */
function protocolInput(input: number): Readable {
  if (!namedPipeOnMacOs(input)) {
    try {
      return new Socket({ fd: input, readable: true, writable: false });
    } catch {
      // not a pipe or a socket
    }
  }
  return createReadStream("", { fd: input, autoClose: false, highWaterMark: 1 << 16 });
}

/** Whether `fd` is a named pipe (one in a file system, not an anonymous pipe) on macOS. */
function namedPipeOnMacOs(fd: number): boolean {
  if (process.platform !== "darwin") {
    return false;
  }
  try {
    const stat = fstatSync(fd);
    return stat.isFIFO() && (stat.nlink > 0 || stat.dev !== 0);
  } catch {
    return false;
  }
}

/** `<label> is spelled <on disk> on disk` (§5.1), or `… is spelled with a different Unicode
 * normalization on disk`. */
function spelledOnDisk({ imported, onDisk, normalization }: Misspelling): string {
  return `${imported} is spelled ${normalization ? "with a different Unicode normalization" : onDisk} on disk`;
}

/** An export of a module namespace; undefined when reading it throws (a binding not yet
 * initialized). */
function exportOf(namespace: Record<string, unknown>, name: string): unknown {
  try {
    return Object.hasOwn(namespace, name) ? namespace[name] : undefined;
  } catch {
    return undefined;
  }
}

function isErrorLike(value: unknown): value is Error {
  if (value instanceof Error) {
    return true;
  }
  if (typeof value !== "object" || value === null) {
    return false;
  }
  try {
    return typeof (value as Error).message === "string" && typeof (value as Error).name === "string";
  } catch {
    return false;
  }
}

function stackOf(error: unknown): string {
  if (isErrorLike(error) && typeof error.stack === "string") {
    return error.stack;
  }
  return safeString(error);
}

/** Writes a line to stderr at once (descriptor 2, never a buffered stream). */
export function log(text: string): void {
  try {
    writeSync(2, `${LOG_PREFIX}${text}\n`);
  } catch {
    // stderr is gone: nothing to tell
  }
}

/** How texts name this package's folder: the folder holding its `package.json`, as the engine
 * named it (the entry's path) and resolved, plain and as a `file:` URL, each written
 * `@grida/fx` (`@grida/fx/package.json`). */
function ownCuts(): (readonly [string, string])[] {
  const folders = new Set<string>([dirname(SDK_FOLDER)]);
  const entry = process.argv[1];
  if (typeof entry === "string" && isAbsolute(entry)) {
    folders.add(dirname(dirname(entry)));
    try {
      folders.add(dirname(dirname(realpathSync(entry))));
    } catch {
      // the entry is gone: its folder as named stays
    }
  }
  const cuts: (readonly [string, string])[] = [];
  for (const folder of folders) {
    cuts.push([`${pathToFileURL(folder).href}/`, `${OWN_NAME}/`]);
    cuts.push([`${folder}${sep}`, `${OWN_NAME}/`]);
  }
  return cuts.sort((a, b) => b[0].length - a[0].length);
}

/**
 * Ends the host, with its process group when it leads one (the engine starts each host as the
 * leader of a group of its own), when the engine that started it is gone: its parent changed, so
 * no end of input may ever come while user code keeps the host busy. A worker thread watches, so
 * it works while a body blocks the main thread; it ends the programs bodies started first, as
 * each runs in a session of its own. The worker's output streams are its own (`stdout: true`,
 * `stderr: true`): it writes to descriptor 2 directly, and the host never sets up its own
 * `process.stdout` or `process.stderr` for it.
 */
function watchEngine(): void {
  // Slot 0 holds how many process groups follow.
  const tools = new Int32Array(new SharedArrayBuffer(4 * (WATCHED_TOOLS + 1)));
  watchTools((groups) => {
    const kept = groups.slice(0, WATCHED_TOOLS);
    kept.forEach((group, index) => Atomics.store(tools, index + 1, group));
    Atomics.store(tools, 0, kept.length);
  });
  const worker = new Worker(
    `
    const { workerData } = require("node:worker_threads");
    const { writeSync } = require("node:fs");
    setInterval(() => {
      if (process.ppid === workerData.engine) return;
      try { writeSync(2, workerData.message); } catch {}
      const tools = workerData.tools;
      const count = Math.min(Atomics.load(tools, 0), tools.length - 1);
      for (let index = 1; index <= count; index += 1) {
        const group = Atomics.load(tools, index);
        if (group > 0) {
          try { process.kill(-group, "SIGKILL"); } catch {}
        }
      }
      let leader = false;
      try { process.kill(-workerData.pid, 0); leader = true; } catch {}
      try { process.kill(leader ? -workerData.pid : workerData.pid, "SIGKILL"); } catch {}
    }, workerData.interval);
    `,
    {
      eval: true,
      workerData: {
        engine: process.ppid,
        pid: process.pid,
        interval: ENGINE_CHECK_INTERVAL_MS,
        message: `${LOG_PREFIX}the engine that started this host is gone; ending\n`,
        tools,
      },
      stdout: true,
      stderr: true,
    },
  );
  worker.unref();
}

/** Serves one session on the protocol descriptors `host-entry.js` made; `exit` is the process's
 * own `process.exit`, which user code cannot call. */
export function main(input: number, output: number, exit: (status: number) => never): void {
  const host = new Host(output, exit);
  host.closures.install(import.meta.url);
  watchEngine();
  // Whatever user code leaves unhandled is logged; it never ends the host.
  process.on("uncaughtException", (error) => log(host.unrooted(stackOf(error))));
  process.on("unhandledRejection", (reason) => {
    log(`unhandled rejection: ${host.unrooted(stackOf(reason))}`);
  });
  // The methods served beside the session's own, before it reads its first message.
  serveAgents(host);
  serveStandIns(host);
  host.listen(input);
}

// -- agents: tool.invoke, agent.check --------------------------------------------------------

/**
 * Serves the engine's requests for the agents of the pending run (§5.4, §5.5): `tool.invoke`
 * calls one of an agent's tools and `agent.check` asks its check, each answered when that is
 * done ({@link Host.serve}: a `$/cancel` answers it `cancelled` at once). Both are served after
 * `shutdown` too: they are part of the pending run.
 */
export function serveAgents(host: Host): void {
  host.runMethods.set("tool.invoke", (id, given) => {
    const params = objectParams(given);
    const agent = agentOf(host, params);
    const callId = params.call_id;
    if (!Object.hasOwn(params, "call_id") || !(callId === null || typeof callId === "string")) {
      throw new Refusal(INVALID_PARAMS, "tool.invoke's call_id is a string or null");
    }
    const name = params.name;
    if (typeof name !== "string" || !name) {
      throw new Refusal(INVALID_PARAMS, "tool.invoke's name is a tool's name");
    }
    const args = params.arguments;
    if (!isPlainObject(args)) {
      throw new Refusal(INVALID_PARAMS, "tool.invoke's arguments are a JSON object");
    }
    return host.serve(id, "tool.invoke", () => agent.serveTool(name, args));
  });
  host.runMethods.set("agent.check", (id, given) => {
    const params = objectParams(given);
    const agent = agentOf(host, params);
    if (!Object.hasOwn(params, "value")) {
      throw new Refusal(INVALID_PARAMS, "agent.check needs the submitted value");
    }
    if (!agent.hasCheck) {
      throw new Refusal(INVALID_PARAMS, `agent ${shown(params.agent_id)} has no check`);
    }
    return host.serve(id, "agent.check", () => agent.serveCheck(params.value));
  });
}

/** The agent of the pending run that `params` name. */
function agentOf(host: Host, params: Record<string, unknown>): AgentLoop {
  const runId = params.run_id;
  const agentId = params.agent_id;
  const pending = host.pendingRun();
  if (pending === null || typeof runId !== "string" || pending.runId !== runId) {
    throw new Refusal(INVALID_PARAMS, `no run ${shown(runId)} is pending on this host`);
  }
  const agent = typeof agentId === "string" ? pending.ctx.agents.get(agentId) : undefined;
  if (agent === undefined) {
    throw new Refusal(INVALID_PARAMS, `run ${runId} has no agent ${shown(agentId)} running`);
  }
  return agent;
}

/** A param as a message names it: a string as it is, anything else as JSON. */
function shown(value: unknown): string {
  return typeof value === "string" ? value : (JSON.stringify(value) ?? "null");
}

// -- stand-ins: stand_in.load, stand_in.answer -----------------------------------------------

/**
 * Makes the host a stand-in host (§5.7) once the engine loads a stand-in: `stand_in.load` imports
 * the file by its absolute path (its imports resolve from its own folder) and takes the export the
 * function names; from then on `stand_in.answer` is answered through an {@link Answerer}, and
 * `describe`, `build` and `run` are refused. A fault of the stand-in is logged on stderr.
 */
export function serveStandIns(host: Host): void {
  let answerer: Answerer | null = null;
  host.methods.set("stand_in.load", async (_id, given) => {
    const params = objectParams(given);
    const path = params.path;
    if (typeof path !== "string" || !path || !isAbsolute(path)) {
      throw new Refusal(INVALID_PARAMS, "stand_in.load's path is the stand-in file's absolute path");
    }
    const name = params.function;
    if (typeof name !== "string" || !IDENTIFIER.test(name)) {
      throw new Refusal(INVALID_PARAMS, "stand_in.load's function is a name");
    }
    if (answerer !== null) {
      throw new Refusal(INVALID_REQUEST, "a stand-in is already loaded");
    }
    if (host.pendingRun() !== null) {
      throw new Refusal(INVALID_REQUEST, "stand_in.load came while a run is pending");
    }
    const named = shownFile(path);
    const namespace = await importStandIn(host, path, named);
    const fn = exportOf(namespace, name);
    if (typeof fn !== "function") {
      throw new Refusal(LOAD_FAILED, `${named} has no function ${name}`);
    }
    answerer = new Answerer(fn as StandIn, {
      errorText: (error) => host.errorText(error),
      onFault: (error) => {
        const data = host.nodeError(error).data as { traceback?: unknown };
        const traceback =
          typeof data.traceback === "string" && data.traceback ? data.traceback : host.errorText(error);
        log(`the stand-in failed:\n${traceback}`);
      },
    });
    host.standingIn = true;
    return {};
  });
  host.methods.set("stand_in.answer", (id, given) => {
    const params = objectParams(given);
    if (answerer === null) {
      throw new Refusal(INVALID_REQUEST, "stand_in.answer came before stand_in.load");
    }
    answerer.start(id, params, (reply) => host.answer(id, reply));
    return LATER;
  });
  host.cancellers.push((id) => answerer !== null && answerer.cancel(id));
}

/** The stand-in file's module namespace; a `load_failed` refusal naming it `shown`. */
async function importStandIn(
  host: Host,
  path: string,
  shown: string,
): Promise<Record<string, unknown>> {
  const file = realPath(path);
  let isFile = false;
  try {
    isFile = statSync(file).isFile();
  } catch {
    isFile = false;
  }
  if (!isFile) {
    throw new Refusal(LOAD_FAILED, `no stand-in file ${shown}`);
  }
  if (!isModuleFile(path)) {
    throw new Refusal(LOAD_FAILED, `cannot import ${shown}`);
  }
  const url = pathToFileURL(file).href;
  try {
    return await host.closures.importModule(url);
  } catch (error) {
    throw new Refusal(LOAD_FAILED, host.importFailure(shown, error));
  }
}

/** How messages name a stand-in file: relative to the working directory (the engine's, where the
 * user named it) when it is inside it, else by its absolute path. */
function shownFile(path: string): string {
  const cwd = process.cwd();
  const given = normalize(path);
  for (const candidate of [given, realPath(given)]) {
    if (candidate !== cwd && within(candidate, cwd)) {
      return relative(cwd, candidate).split(sep).join("/");
    }
  }
  return given;
}
