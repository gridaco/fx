/**
 * What a node body receives: {@link Ctx}, over the node protocol (`spec/protocol.md` §5.3, §6).
 *
 * The host builds one for each `run` from its params and a {@link Channel} to the engine:
 *
 * - `instance`, `inputs` and `params`: what the run was given; a keyed input is a `ReadonlyMap`
 *   in collection order, and each `param_files` entry is put back into `params` at its JSON
 *   pointer as an `InputFile`. The host checks the params against their declared schemas before
 *   the body runs ({@link RunContext.checkParams}): a null for an optional param is left out.
 * - `signal`: aborted by `$/cancel` (a timeout, or a person stopping the run); the body decides
 *   what to do with it.
 * - `read`: local reads of an input file (strict UTF-8 text, JSON, bytes); `out`: files the body
 *   makes, which leave the host by `file.put` (or by their work path) when they are returned, put
 *   in a request, or shown to an agent.
 * - `fact`, `annotate` and `progress` report to the engine, and `facts` and `marks` mirror what
 *   was reported; `prompt`, `capability` (and its sugar) and `tool` use the engine's services.
 *   A value the engine could not read is refused before anything is sent; an object member whose
 *   value is `undefined` is left out, as `JSON.stringify` leaves it out.
 * - `tool(name).run`: a program in a session of its own, ended with everything it started at its
 *   timeout, when `signal` aborts, and when the run ends.
 * - `agent`: a tool-using model loop the engine runs (`agent.run`), whose tools and check the host
 *   serves while it runs ({@link AgentLoop}).
 * - `state`: shared with the body's agent tools, never sent; `fail(message)` makes a
 *   `NodeFailure` to throw.
 */

import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, realpathSync, statSync } from "node:fs";
import { constants } from "node:os";
import { readFile } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import {
  CallFailed,
  EngineError,
  isEngineError,
  isNodeFailure,
  NodeFailure,
} from "./engine-errors.js";
import {
  DIGEST,
  decodeText,
  type FileRefWire,
  fileRefOf,
  InputFile,
  makeOutput,
  type Output,
  type OutputSource,
  outputSourceOf,
} from "./files.js";
import { familyOf, kindOf, suffixOf } from "./kinds.js";
import {
  type FromSchema,
  type InputsOf,
  type JsonSchemaObject,
  type NodeDecl,
  type ParamsOf,
  type Picture,
  type Ports,
  SUBMIT,
  type Tool,
  type ToolRecord,
  toolRecordOf,
  toolReplyOf,
} from "./node.js";
import {
  checkValue,
  isPlainObject,
  NODE_ERROR,
  NODE_FAILURE,
  placeAt,
  pyRepr,
  safeString,
  setOwn,
  typeName,
} from "./protocol.js";
import { checkedParams } from "./schema.js";
import { errorText } from "./stand-in.js";
import type { JsonInput, JsonValue } from "./workflow.js";

/** `ctx.instance`: the instance being run (`spec/identity.md` §11). */
export interface Instance {
  /** The instance id: `draw#1`. */
  readonly id: string;
  /** The step path with repeat keys. */
  readonly path: string;
  /** The declared step path, without keys. */
  readonly step: string;
  /** The repeat key, or null. */
  readonly key: string | null;
  /** One take number per regenerating level, outermost first. */
  readonly takes: readonly number[];
  /** The last of `takes`. */
  readonly take: number;
}

/** One annotation mark (`spec/protocol.md` §6.4): coordinates are fractions of the image. No
 * shape is a note about the whole image. */
export interface Mark {
  readonly shape?: "point" | "points" | "box" | undefined;
  /** A point's `[x, y]`. */
  readonly at?: readonly [number, number] | undefined;
  /** A polyline's (or, with `closed`, a polygon's) points. */
  readonly points?: readonly (readonly [number, number])[] | undefined;
  readonly closed?: boolean | undefined;
  /** A box's `[x0, y0, x1, y1]`. */
  readonly box?: readonly [number, number, number, number] | undefined;
  readonly label?: string | undefined;
  readonly color?: string | undefined;
  readonly tag?: string | undefined;
  /** Any other field: a JSON value (`undefined` leaves it out). */
  readonly [field: string]: JsonInput | undefined;
}

/** A value of a capability request or of prompt variables: JSON, with files anywhere in it (an
 * `InputFile` or an `Output`), which travel as file values. An object member whose value is
 * `undefined` is left out. */
export type RequestValue =
  | JsonInput
  | InputFile
  | Output
  | readonly RequestValue[]
  | { readonly [key: string]: RequestValue | undefined };

/** A capability request, or a prompt's variables: an object of {@link RequestValue}s. */
export type RequestObject = { readonly [member: string]: RequestValue | undefined };

/** A capability's answer (`spec/protocol.md` §6.1). */
export interface CallResult {
  /** The call key. */
  readonly key: string | null;
  /** Whether the call cache answered. */
  readonly cached: boolean;
  /** What this call cost; null when nothing was billed. */
  readonly costUsd: number | null;
  /** The answer's files, by name. */
  readonly files: ReadonlyMap<string, InputFile>;
  readonly data: JsonValue;
  /** The first file of the family; a `CallFailed` (`the call returned no image`) when there is
   * none. */
  readonly image: InputFile;
  readonly audio: InputFile;
  readonly video: InputFile;
  /** `data.json` when `data` is an object holding `json` (a `structured.generate` answer), else
   * `data`. */
  readonly json: unknown;
}

/** How `ToolHandle.run` runs the program. */
export interface ToolRunOptions {
  /** Where it starts (default: the run's work folder). */
  readonly cwd?: string;
  /** At this many seconds (a number above 0) the program and everything it started end, and the
   * node fails with `<name> ran past <s> seconds`; null or left out, it runs as long as it takes,
   * until `ctx.signal` aborts or the run ends. */
  readonly timeoutS?: number | null | undefined;
  /** Replaces the environment when given. */
  readonly env?: { readonly [name: string]: string };
  /** With `check` (the default) a non-zero exit fails the node with `<name> exited <code>: ` and
   * the last 2000 characters of its stderr (or stdout). */
  readonly check?: boolean;
}

/** How a program run by `ToolHandle.run` ended: its exit status (minus the signal's number when a
 * signal ended it) and what it printed. */
export interface ToolRun {
  readonly status: number;
  readonly stdout: string;
  readonly stderr: string;
}

/** `ctx.tool(name)`: a declared external program. */
export interface ToolHandle {
  readonly name: string;
  /** The program the engine found (`GRIDA_FX_TOOL_<NAME>`, else `PATH`). */
  readonly executable: string;
  /** Where `run` starts it when no `cwd` is given: the run's work folder. */
  readonly workDir: string;
  /** Runs the program with `argv` in a session of its own, standard input closed. When
   * `ctx.signal` aborts, the program and everything it started end and `run` rejects with the
   * signal's reason; whatever still runs when the run ends is ended too. */
  run(argv: readonly string[], options?: ToolRunOptions): Promise<ToolRun>;
}

/** The names of a declaration's inputs of exactly one file: not optional, not a list, not keyed
 * (any name for a context of no declaration in particular). */
type InputName<D extends NodeDecl> =
  NonNullable<D["inputs"]> extends infer I extends Ports
    ? string extends keyof I
      ? string
      : {
          [K in keyof I]: I[K] extends `${string}?` | `${string}[]` | `${string}{}` ? never : K;
        }[keyof I] &
          string
    : never;

/** `ctx.read`: local reads of an input file, given as an `InputFile` or by the name of an input
 * that is always one file. */
export interface Reader<D extends NodeDecl = NodeDecl> {
  bytes(file: InputFile | InputName<D>): Promise<Uint8Array>;
  /** Strict UTF-8, one leading byte-order mark dropped, line endings kept. */
  text(file: InputFile | InputName<D>): Promise<string>;
  json(file: InputFile | InputName<D>): Promise<unknown>;
}

/** `ctx.out`: files the body makes. Each is an {@link Output} to return, put in a request, or
 * show to an agent. */
export interface Maker {
  /** An encoded PNG (bodies bring their own encoder). */
  png(data: Uint8Array): Output;
  bytes(data: Uint8Array, kind: string): Output;
  /** UTF-8 text; `kind` defaults to `text/plain`. */
  text(text: string, kind?: string): Output;
  /** A JSON value, which the engine writes in its canonical format (`spec/identity.md` §5). */
  json(value: JsonInput): Output;
  /** A path under the work folder's `out/` (parents made), to write a file to. */
  path(name: string): string;
  /** A file the body wrote: one under the work folder leaves by its work path (read when it
   * leaves); any other file's bytes are read now. `kind` defaults to the suffix table. */
  file(path: string, kind?: string): Output;
}

/** What a node body receives (module comment). */
export interface Ctx<D extends NodeDecl = NodeDecl> {
  readonly instance: Instance;
  readonly inputs: InputsOf<D>;
  readonly params: ParamsOf<D>;
  /** Aborted by `$/cancel`: a timeout, or a person stopping the run. */
  readonly signal: AbortSignal;
  /** Shared with the body's agent tools; never sent. */
  readonly state: Record<string, unknown>;
  /** The node facts reported with `fact`. */
  readonly facts: Readonly<Record<string, JsonValue>>;
  /** The marks reported with `annotate`. */
  readonly marks: readonly Mark[];
  /** The run's work folder: the body may write anything under it. */
  readonly workDir: string;
  /** A path under the work folder (parents made); a `NodeFailure` for a name that leaves it. */
  workPath(name: string): string;
  readonly read: Reader<D>;
  readonly out: Maker;
  /** Reports a node fact; the engine records it at once. */
  fact(name: string, value: JsonInput): Promise<void>;
  /** Adds one mark to this node's annotations. */
  annotate(mark: Mark): Promise<void>;
  /** Shows progress (`fraction` from 0 to 1). */
  progress(text: string, fraction?: number): void;
  /** Renders a declared resource with `${{ }}` over the params, `variables` on top. */
  prompt(path: string, variables?: RequestObject): Promise<string>;
  /** A paid call through the engine: routed, priced, budgeted, retried, recorded and cached.
   * Files anywhere in the request (an `InputFile`, an `Output`) travel as file values. */
  capability(capability: string, request: RequestObject): Promise<CallResult>;
  imageGenerate(request: RequestObject): Promise<CallResult>;
  imageEdit(request: RequestObject): Promise<CallResult>;
  structuredGenerate(request: RequestObject): Promise<CallResult>;
  /** A declared external program. */
  tool(name: string): ToolHandle;
  /** A tool-using model loop the engine runs for the body (`agent.run`), with these tools. */
  agent(options: AgentOptions): Agent;
  /** A `NodeFailure` to throw: `throw ctx.fail("…")`. */
  fail(message: string): NodeFailure;
}

/** `ctx.agent(options)`. */
export interface AgentOptions {
  /** The system prompt. */
  readonly system: string;
  /** The tools the model may call, each made by `tool()`. */
  readonly tools?: readonly Tool[];
  /** Each turn shows the model only the newest this many pictures. */
  readonly recentImages?: number;
  /** The most tokens of each turn's reply. */
  readonly maxTokens?: number;
}

/** What a check returns: a refusal, or nothing to accept the value. */
export type CheckResult = string | null | undefined | void;

/** `agent.run`'s options. */
export interface AgentRunOptions {
  /** The most turns, at least 1: past them the run fails with `AgentUnfinished`. */
  readonly maxSteps: number;
  /** Pictures for the opening message. */
  readonly images?: readonly Picture[];
}

/**
 * A tool-using model loop the engine runs (`spec/protocol.md` §6.2): its tools are served while
 * a `run` is pending. Each `run` starts a new transcript; {@link transcript} holds the last one,
 * also when the run failed after its loop started, so a body that catches the failure can read
 * what the agent tried.
 */
export interface Agent {
  /** Runs the loop until the model answers; its text. */
  run(
    instructions: string,
    options: AgentRunOptions & { readonly submit?: undefined; readonly check?: undefined },
  ): Promise<string>;
  /** Runs the loop until the model submits a value that meets `submit`, a JSON Schema, and that
   * `check` accepts (it returns a refusal, or nothing; a `NodeFailure` it throws is a refusal
   * too, and anything else it throws fails the node); the value. */
  run<const S extends JsonSchemaObject>(
    instructions: string,
    options: AgentRunOptions & {
      readonly submit: S;
      readonly check?: (value: FromSchema<S>) => CheckResult | Promise<CheckResult>;
    },
  ): Promise<FromSchema<S>>;
  /** The last run's messages, every one, as the engine reports them. */
  readonly transcript: readonly JsonValue[];
}

/** The engine, for one run: requests carry the run's `run_id`. */
export interface Channel {
  /** Sends a request and waits for its result; the engine's error is an `EngineError`. A
   * `TypeError` (nothing sent) for params the engine could not read. */
  request(method: string, params: Record<string, unknown>): Promise<unknown>;
  /** Sends a notification (`progress`). */
  notify(method: string, params: Record<string, unknown>): void;
}

/** The `run` params a context is made from (`spec/protocol.md` §5.3), already checked. */
export interface RunParams {
  readonly run_id: string;
  readonly instance: {
    readonly id: string;
    readonly path: string;
    readonly step: string;
    readonly key?: string | null;
    readonly take: readonly number[];
  };
  readonly params: Record<string, unknown>;
  readonly param_files: Record<string, FileRefWire>;
  readonly inputs: Record<string, unknown>;
  readonly work_dir: string;
  readonly resources: Record<string, unknown>;
  readonly tools: Record<string, unknown>;
  readonly calls: Record<string, unknown>;
  readonly timeout_s?: number | null;
}

const SHAPES = ["point", "points", "box"] as const;
const GEOMETRY = { point: "at", points: "points", box: "box" } as const;
const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/** A context for one run (module comment). The host reads its internals: `kept()` for a failure's
 * data, `put()` to store an output, `agents` for the engine's agent requests. */
export class RunContext implements Omit<Ctx, "inputs" | "params"> {
  readonly run: RunParams;
  readonly channel: Channel;
  readonly instance: Instance;
  readonly state: Record<string, unknown> = {};
  readonly facts: Record<string, JsonValue> = {};
  readonly marks: Mark[] = [];
  readonly workDir: string;
  readonly read: Reader;
  readonly out: Maker;
  readonly #controller = new AbortController();
  /** The params once checked against their schemas ({@link checkParams}). */
  #given: Record<string, unknown> | undefined;
  #params: Record<string, unknown> | undefined;
  #inputs: Record<string, unknown> | undefined;
  #realWorkDir: string | undefined;
  /** Outputs already stored with `file.put` in this run, by identity (work files excepted: they
   * may change after they are stored). */
  readonly #stored = new Map<Output, FileRefWire>();
  /** Agents whose `agent.run` is pending, by `agent_id` (`tool.invoke` finds them). */
  readonly agents = new Map<string, AgentLoop>();
  #agentCount = 0;
  /** The process groups of this run's programs that still run. */
  readonly #tools = new Set<number>();
  #toolsEnded = false;
  /** Cuts the project root from a text the model is told (a tool's error). */
  readonly unrooted: (text: string) => string;

  constructor(run: RunParams, channel: Channel, unrooted: (text: string) => string = (text) => text) {
    this.run = run;
    this.channel = channel;
    this.unrooted = unrooted;
    const takes = Object.freeze([...run.instance.take]);
    this.instance = Object.freeze({
      id: run.instance.id,
      path: run.instance.path,
      step: run.instance.step,
      key: typeof run.instance.key === "string" ? run.instance.key : null,
      takes,
      take: takes.length > 0 ? (takes[takes.length - 1] as number) : 1,
    });
    this.workDir = run.work_dir;
    this.read = makeReader(this);
    this.out = makeMaker(this);
  }

  get signal(): AbortSignal {
    return this.#controller.signal;
  }

  /** Aborts `signal` (`$/cancel` for this run). */
  cancel(reason: unknown): void {
    this.#controller.abort(reason);
  }

  /** Checks the params against their declared schemas before the body runs (`schema.ts`): a
   * value that does not meet its schema is a `NodeFailure`, and a null for an optional param is
   * left out. */
  checkParams(declared: Readonly<Record<string, unknown>>): void {
    const files: Record<string, string> = {};
    for (const [pointer, ref] of Object.entries(this.run.param_files ?? {})) {
      setOwn(files, pointer, String(ref.digest));
    }
    this.#given = checkedParams(this.run.params ?? {}, declared, files);
    this.#params = undefined;
  }

  get params(): Record<string, unknown> {
    if (this.#params === undefined) {
      const params = structuredClone(this.#given ?? this.run.params ?? {});
      for (const [pointer, ref] of Object.entries(this.run.param_files ?? {})) {
        putAt(params, pointer, new InputFile(ref));
      }
      this.#params = params;
    }
    return this.#params;
  }

  get inputs(): Record<string, unknown> {
    if (this.#inputs === undefined) {
      const inputs: Record<string, unknown> = {};
      for (const [name, value] of Object.entries(this.run.inputs ?? {})) {
        inputs[name] = staged(value as Record<string, unknown>);
      }
      this.#inputs = inputs;
    }
    return this.#inputs;
  }

  /** The work folder, its symbolic links resolved. */
  realWorkDir(): string {
    this.#realWorkDir ??= realPath(this.run.work_dir);
    return this.#realWorkDir;
  }

  workPath(name: string): string {
    const workDir = this.realWorkDir();
    const path = realPath(resolve(workDir, String(name)));
    if (!within(path, workDir)) {
      throw new NodeFailure(`${name} is outside this node's work folder`);
    }
    mkdirSync(dirname(path), { recursive: true });
    return path;
  }

  // -- reporting ---------------------------------------------------------------------------

  async fact(name: string, value: JsonInput): Promise<void> {
    if (typeof name !== "string") {
      throw new TypeError(`a fact's name is a string, not ${typeName(name)}`);
    }
    try {
      checkValue(value);
    } catch (error) {
      throw new TypeError(`fact ${name}: ${(error as Error).message}`);
    }
    await this.channel.request("fact", { name, value });
    setOwn(this.facts, name, value as JsonValue);
  }

  async annotate(mark: Mark): Promise<void> {
    if (!isPlainObject(mark)) {
      throw new TypeError(`a mark is an object, not ${typeName(mark)}`);
    }
    const { shape, label, color, tag, ...fields } = mark as Record<string, unknown>;
    const made: Record<string, unknown> = {};
    if (shape !== undefined && shape !== null) {
      if (!(SHAPES as readonly unknown[]).includes(shape)) {
        throw new NodeFailure(`a mark's shape is one of ${SHAPES.join(", ")}`);
      }
      made.shape = shape;
      const geometry = GEOMETRY[shape as (typeof SHAPES)[number]];
      if (fields[geometry] === undefined) {
        throw new NodeFailure(`a ${String(shape)} mark needs ${geometry}`);
      }
    }
    for (const [field, value] of [
      ["label", label],
      ["color", color],
      ["tag", tag],
    ] as const) {
      if (value !== undefined && value !== null) {
        made[field] = value;
      }
    }
    for (const [field, value] of Object.entries(fields)) {
      if (value !== undefined) {
        made[field] = value;
      }
    }
    try {
      checkValue(made);
    } catch (error) {
      throw new TypeError(`a mark: ${(error as Error).message}`);
    }
    await this.channel.request("annotate", { mark: made });
    this.marks.push(made as Mark);
  }

  progress(text: string, fraction?: number): void {
    const params: Record<string, unknown> = { text };
    if (fraction !== undefined && fraction !== null) {
      params.fraction = fraction;
    }
    this.channel.notify("progress", params);
  }

  fail(message: string): NodeFailure {
    return new NodeFailure(message);
  }

  // -- the engine's services ---------------------------------------------------------------

  async prompt(path: string, variables: RequestObject = {}): Promise<string> {
    if (!Object.hasOwn(this.run.resources ?? {}, path)) {
      throw new NodeFailure(`${path} is not one of this node's declared resources`);
    }
    const stored = await this.putAll(outputsIn(variables), "prompt/file");
    const answer = (await this.channel.request("prompt.render", {
      path,
      variables: withFiles(variables, stored),
    })) as { text?: unknown } | null;
    return String(answer?.text);
  }

  async capability(capability: string, request: RequestObject): Promise<CallResult> {
    const stored = await this.putAll(outputsIn(request), "request/file");
    const answer = await this.channel.request("capability", {
      capability,
      request: withFiles(request, stored),
    });
    return new Called((answer ?? {}) as Record<string, unknown>);
  }

  imageGenerate(request: RequestObject): Promise<CallResult> {
    return this.capability("image.generate", request);
  }

  imageEdit(request: RequestObject): Promise<CallResult> {
    return this.capability("image.edit", request);
  }

  structuredGenerate(request: RequestObject): Promise<CallResult> {
    return this.capability("structured.generate", request);
  }

  tool(name: string): ToolHandle {
    const tools = this.run.tools ?? {};
    if (!Object.hasOwn(tools, name)) {
      throw new NodeFailure(`${name} is not one of this node's declared tools`);
    }
    const executable = tools[name];
    if (typeof executable !== "string") {
      throw new NodeFailure(`${name} is not installed (see grida-fx doctor)`);
    }
    return new Program(name, executable, this.workPath("."), this);
  }

  agent(options: AgentOptions): Agent {
    return new AgentLoop(this, options);
  }

  // -- files leaving the host --------------------------------------------------------------

  /** Stores `output` with `file.put` (once per run, unless it is a work file) and returns its
   * file ref; `name` is the display name the engine records. */
  async put(output: Output, name: string): Promise<FileRefWire> {
    const known = this.#stored.get(output);
    if (known !== undefined) {
      return known;
    }
    const source = outputSourceOf(output) as OutputSource;
    const params: Record<string, unknown> = {};
    if ("workPath" in source) {
      params.work_path = source.workPath;
    } else if ("data" in source) {
      params.base64 = Buffer.from(source.data).toString("base64");
    } else {
      params.json = source.json;
    }
    params.kind = output.kind;
    params.name = name;
    const ref = (await this.channel.request("file.put", params)) as FileRefWire;
    if (!("workPath" in source)) {
      this.#stored.set(output, ref);
    }
    return ref;
  }

  /** Stores each of `outputs`; their digests by output. */
  async putAll(outputs: readonly Output[], name: string): Promise<Map<Output, string>> {
    const stored = new Map<Output, string>();
    for (const output of outputs) {
      stored.set(output, (await this.put(output, name)).digest);
    }
    return stored;
  }

  /** The node facts and marks the body reported, for a failure's `data`: either is left out when
   * the body made it something that is not an I-JSON value. */
  kept(): Record<string, unknown> {
    const kept: Record<string, unknown> = {};
    const facts: Record<string, unknown> = {};
    for (const [name, value] of Object.entries(this.facts)) {
      if (name !== "cost_usd") {
        setOwn(facts, name, value);
      }
    }
    for (const [key, value] of [
      ["facts", facts],
      ["marks", [...this.marks]],
    ] as const) {
      try {
        checkValue(value);
      } catch {
        continue;
      }
      kept[key] = value;
    }
    return kept;
  }

  /** The next agent's id: `agent-1`, `agent-2`, … */
  nextAgentId(): string {
    this.#agentCount += 1;
    return `agent-${this.#agentCount}`;
  }

  // -- programs the body runs ----------------------------------------------------------------

  /** Keeps the process group of a program `ctx.tool(…).run` started, to end it with the run; one
   * started once the run's programs were ended is ended at once. */
  startedTool(group: number): void {
    if (this.#toolsEnded) {
      endGroup(group);
      return;
    }
    this.#tools.add(group);
    runningTools.add(group);
    toolsChanged();
  }

  /** A program that ended by itself. */
  endedTool(group: number): void {
    this.#tools.delete(group);
    runningTools.delete(group);
    toolsChanged();
  }

  /** Ends every program of this run still running, with everything each started (the run is
   * over). */
  endTools(): void {
    this.#toolsEnded = true;
    for (const group of this.#tools) {
      endGroup(group);
      runningTools.delete(group);
    }
    this.#tools.clear();
    toolsChanged();
  }
}

/** The process groups of every program a body started that still runs, in any run. */
const runningTools = new Set<number>();
let onToolsChanged: ((groups: readonly number[]) => void) | null = null;

function toolsChanged(): void {
  onToolsChanged?.([...runningTools]);
}

/** Tells `listener` the process groups of the programs still running whenever they change (the
 * host's orphan watch ends them with the host). */
export function watchTools(listener: (groups: readonly number[]) => void): void {
  onToolsChanged = listener;
  toolsChanged();
}

/** Ends every program a body started that still runs (the host is ending). */
export function endAllTools(): void {
  for (const group of runningTools) {
    endGroup(group);
  }
  runningTools.clear();
  toolsChanged();
}

/** A staged input (`spec/protocol.md` §3.3) as the body sees it. */
function staged(value: Record<string, unknown>): unknown {
  if (Object.hasOwn(value, "list") && !Object.hasOwn(value, "digest")) {
    return (value.list as FileRefWire[]).map((ref) => new InputFile(ref));
  }
  if (Object.hasOwn(value, "collection") && !Object.hasOwn(value, "digest")) {
    return new Map(
      (value.collection as [string, FileRefWire][]).map(([key, ref]) => [key, new InputFile(ref)]),
    );
  }
  return new InputFile(value as unknown as FileRefWire);
}

/** Puts `file` at the RFC 6901 `pointer` into `params`, at a place they have. */
function putAt(params: Record<string, unknown>, pointer: string, file: InputFile): void {
  if (!pointer.startsWith("/")) {
    const shown = JSON.stringify(pointer);
    throw new TypeError(`param_files names ${shown}, which is not a pointer into params`);
  }
  if (!placeAt(params, pointer, file)) {
    throw new TypeError(`param_files names ${pointer}, which is not in params`);
  }
}

/** Every {@link Output} inside arrays and plain objects of `value`, each once. */
function outputsIn(value: unknown, found: Output[] = []): Output[] {
  if (outputSourceOf(value) !== undefined) {
    if (!found.includes(value as Output)) {
      found.push(value as Output);
    }
  } else if (Array.isArray(value)) {
    for (const item of value) {
      outputsIn(item, found);
    }
  } else if (isPlainObject(value)) {
    for (const item of Object.values(value)) {
      outputsIn(item, found);
    }
  }
  return found;
}

/** `value` with every file in it as a file value (`spec/protocol.md` §3.2): an `InputFile` by its
 * digest, an `Output` by the digest it was stored under. */
export function withFiles(value: unknown, stored: ReadonlyMap<Output, string>): unknown {
  const ref = fileRefOf(value);
  if (ref !== undefined) {
    return { file: ref.digest };
  }
  if (outputSourceOf(value) !== undefined) {
    return { file: stored.get(value as Output) };
  }
  if (Array.isArray(value)) {
    return value.map((item) => withFiles(item, stored));
  }
  if (isPlainObject(value)) {
    const converted: Record<string, unknown> = {};
    for (const [key, item] of Object.entries(value)) {
      setOwn(converted, key, withFiles(item, stored));
    }
    return converted;
  }
  return value;
}

const AGENT_OPTIONS = ["system", "tools", "recentImages", "maxTokens"];
const RUN_OPTIONS = ["maxSteps", "images", "submit", "check"];
/** A picture's kind by its suffix; anything else is sent as a PNG. */
const PICTURE_KINDS: Readonly<Record<string, string>> = {
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".webp": "image/webp",
};
const HTTP_URL = /^https?:\/\//i;

/**
 * `ctx.agent(…)` ({@link Agent}). The host serves the engine's requests for a pending `run` through
 * {@link serveTool} (`tool.invoke`) and {@link serveCheck} (`agent.check`). A tool's `NodeFailure`
 * (or what a check throws that is not one) ends the loop: the engine then fails `agent.run` with
 * that error, and `run` throws the tool's or the check's own error, so the body sees what its code
 * threw.
 *
 * Pictures leave the host as file values (`spec/protocol.md` §3.2): an `InputFile` by its digest;
 * an `Output` stored with `file.put` (`agent/image`); a `data:` URL decoded and stored (its media
 * type the kind, `image/png` without one); a path or a `file:` URL by its work path when it is a
 * file under the work folder, else by its bytes (kind by suffix: `.png`, `.jpg`/`.jpeg`, `.webp`,
 * else `image/png`), named by its file name. An `http(s)` URL is refused: the model is shown
 * files.
 */
export class AgentLoop implements Agent {
  readonly #ctx: RunContext;
  readonly #system: string;
  readonly #tools = new Map<string, ToolRecord>();
  readonly #recentImages: unknown;
  readonly #maxTokens: unknown;
  #transcript: JsonValue[] = [];
  #check: ((value: unknown) => unknown) | null = null;
  /** What a tool or the check threw that ended the loop, thrown again by `run`. */
  #endedBy: { readonly error: unknown } | null = null;

  constructor(ctx: RunContext, options: AgentOptions) {
    if (!isPlainObject(options)) {
      throw new TypeError(`ctx.agent takes an object of options, not ${typeName(options)}`);
    }
    for (const key of Object.keys(options)) {
      if (!AGENT_OPTIONS.includes(key)) {
        throw new TypeError(
          `ctx.agent takes system, tools, recentImages and maxTokens, not ${pyRepr(key)}`,
        );
      }
    }
    const { system, tools = [], recentImages, maxTokens } = options as Record<string, unknown>;
    if (typeof system !== "string") {
      throw new TypeError(`an agent's system prompt is a string, not ${typeName(system)}`);
    }
    if (!Array.isArray(tools)) {
      throw new TypeError(`an agent's tools are a list of tool() declarations, not ${typeName(tools)}`);
    }
    for (const item of tools) {
      const record = toolRecordOf(item);
      if (record === undefined) {
        throw new NodeFailure(`${shownTool(item)} is not declared with tool()`);
      }
      if (record.name === SUBMIT) {
        throw new NodeFailure(`${SUBMIT} is the agent's own tool; name yours otherwise`);
      }
      // A second tool of one name replaces the first.
      this.#tools.set(record.name, record);
    }
    this.#ctx = ctx;
    this.#system = system;
    this.#recentImages = recentImages;
    this.#maxTokens = maxTokens;
  }

  get transcript(): readonly JsonValue[] {
    return this.#transcript;
  }

  /** Whether the pending `run` has a check (`agent.check` is served). */
  get hasCheck(): boolean {
    return this.#check !== null;
  }

  run(
    instructions: string,
    options: AgentRunOptions & { readonly submit?: undefined; readonly check?: undefined },
  ): Promise<string>;
  run<const S extends JsonSchemaObject>(
    instructions: string,
    options: AgentRunOptions & {
      readonly submit: S;
      readonly check?: (value: FromSchema<S>) => CheckResult | Promise<CheckResult>;
    },
  ): Promise<FromSchema<S>>;
  async run(instructions: string, options: AgentRunOptions): Promise<unknown> {
    if (!isPlainObject(options)) {
      throw new TypeError(`agent.run takes an object of options, not ${typeName(options)}`);
    }
    for (const key of Object.keys(options)) {
      if (!RUN_OPTIONS.includes(key)) {
        throw new TypeError(`agent.run takes maxSteps, images, submit and check, not ${pyRepr(key)}`);
      }
    }
    const { maxSteps, images = [], submit, check } = options as Record<string, unknown>;
    if (typeof maxSteps !== "number" || !Number.isInteger(maxSteps) || maxSteps < 1) {
      throw new TypeError(`maxSteps is a whole number of at least 1, not ${pyRepr(maxSteps)}`);
    }
    if (typeof instructions !== "string") {
      throw new TypeError(`an agent's instructions are a string, not ${typeName(instructions)}`);
    }
    if (!Array.isArray(images)) {
      throw new TypeError(`images is a list of pictures, not ${typeName(images)}`);
    }
    const submits = submit !== undefined && submit !== null;
    if (submits && !isPlainObject(submit)) {
      throw new TypeError(`submit is a JSON Schema object, not ${typeName(submit)}`);
    }
    if (check !== undefined && check !== null && typeof check !== "function") {
      throw new TypeError(`check is a function, not ${typeName(check)}`);
    }
    const ctx = this.#ctx;
    const pictures: { file: string }[] = [];
    for (const image of images) {
      pictures.push(await this.#picture(image));
    }
    const agentId = ctx.nextAgentId();
    // The members in agent.run's order (`spec/protocol.md` §6.2); the optional ones only when set.
    const params: Record<string, unknown> = { agent_id: agentId, system: this.#system, instructions };
    if (pictures.length > 0) {
      params.images = pictures;
    }
    params.tools = [...this.#tools.values()].map(({ name, description, parameters }) => ({
      name,
      description,
      parameters,
    }));
    params.max_steps = maxSteps;
    params.submit = submits ? submit : null;
    const checked = submits && typeof check === "function";
    params.check = checked;
    if (this.#recentImages !== undefined && this.#recentImages !== null) {
      params.recent_images = this.#recentImages;
    }
    if (this.#maxTokens !== undefined && this.#maxTokens !== null) {
      params.max_tokens = this.#maxTokens;
    }
    this.#check = checked ? (check as (value: unknown) => unknown) : null;
    this.#endedBy = null;
    this.#transcript = [];
    ctx.agents.set(agentId, this);
    let result: unknown;
    try {
      result = await ctx.channel.request("agent.run", params);
    } catch (error) {
      if (isEngineError(error)) {
        this.#transcript = transcriptOf(error.data);
        // Set meanwhile by a tool or the check that ended the loop.
        const ended = this.#endedBy as { readonly error: unknown } | null;
        if (ended !== null && (error.code === NODE_FAILURE || error.code === NODE_ERROR)) {
          throw ended.error;
        }
      }
      throw error;
    } finally {
      ctx.agents.delete(agentId);
      this.#check = null;
      this.#endedBy = null;
    }
    if (!isPlainObject(result)) {
      throw new EngineError(`the engine answered agent.run with ${typeName(result)}`);
    }
    this.#transcript = transcriptOf(result);
    if (Object.hasOwn(result, "submitted")) {
      return result.submitted;
    }
    return typeof result.text === "string" ? result.text : "";
  }

  /** Calls one tool for `tool.invoke` (`spec/protocol.md` §5.4): its answer. A `NodeFailure` it
   * throws is thrown (the host answers `node_failure`). */
  async serveTool(name: string, args: Record<string, unknown>): Promise<Record<string, unknown>> {
    const record = this.#tools.get(name);
    if (record === undefined) {
      return { content: `no tool named ${name}` };
    }
    try {
      const handler = record.handler as (args: unknown, ctx: Ctx) => unknown;
      return await this.#reply(await handler(args, this.#ctx as unknown as Ctx));
    } catch (error) {
      if (isNodeFailure(error)) {
        this.#endedBy = { error };
        throw error;
      }
      return { error: this.#ctx.unrooted(errorText(error)) };
    }
  }

  /** Asks the check about a submitted value for `agent.check` (`spec/protocol.md` §5.5): the
   * string it returned, or the message of a `NodeFailure` it threw, is the refusal. Anything else
   * it throws, or a return value that is neither a string nor nothing, is thrown (the host answers
   * `node_error`). */
  async serveCheck(value: unknown): Promise<{ refusal: string | null }> {
    const check = this.#check;
    if (check === null) {
      return { refusal: null };
    }
    let returned: unknown;
    try {
      returned = await check(value);
    } catch (error) {
      if (isNodeFailure(error)) {
        return { refusal: String(error.message ?? "") };
      }
      this.#endedBy = { error };
      throw error;
    }
    if (typeof returned === "string") {
      return { refusal: returned };
    }
    if (returned === undefined || returned === null) {
      return { refusal: null };
    }
    const error = new TypeError(
      `a check returns a refusal (a string) or nothing, not ${typeName(returned)}`,
    );
    this.#endedBy = { error };
    throw error;
  }

  /** What a tool returned, as `tool.invoke`'s answer. */
  async #reply(result: unknown): Promise<Record<string, unknown>> {
    const reply = toolReplyOf(result);
    if (reply !== undefined) {
      const answer: Record<string, unknown> = {
        content: typeof reply.text === "string" ? reply.text : safeString(reply.text),
      };
      if (reply.images.length > 0) {
        const images: { file: string }[] = [];
        for (const image of reply.images) {
          images.push(await this.#picture(image));
        }
        answer.images = images;
      }
      return answer;
    }
    if (result === undefined) {
      return { content: null };
    }
    try {
      // The content sits in the result of a response: two levels inside its message.
      checkValue(result, "", 2);
    } catch {
      return { content: safeString(result) };
    }
    return { content: result };
  }

  /** A picture as a file value (class comment). */
  async #picture(image: unknown): Promise<{ file: string }> {
    const ctx = this.#ctx;
    const ref = fileRefOf(image);
    if (ref !== undefined) {
      return { file: ref.digest };
    }
    if (outputSourceOf(image) !== undefined) {
      return { file: (await ctx.put(image as Output, "agent/image")).digest };
    }
    if (isPlainObject(image)) {
      const keys = Object.keys(image);
      if (keys.length === 1 && typeof image.file === "string" && DIGEST.test(image.file)) {
        return { file: image.file };
      }
    }
    let path: string | null = null;
    if (image instanceof URL) {
      if (image.protocol === "data:") {
        return this.#dataUrl(image.href);
      }
      if (image.protocol !== "file:") {
        throw new NodeFailure("an agent picture is a file, not a URL");
      }
      path = fileURLToPath(image);
    } else if (typeof image === "string") {
      if (image.startsWith("data:")) {
        return this.#dataUrl(image);
      }
      if (HTTP_URL.test(image)) {
        throw new NodeFailure("an agent picture is a file, not a URL");
      }
      path = image;
    }
    if (path === null) {
      throw new NodeFailure(`an agent picture is a file, not ${typeName(image)}`);
    }
    const name = basename(path);
    const kind = PICTURE_KINDS[suffixOf(path)] ?? "image/png";
    const resolved = realPath(resolve(path));
    const workDir = ctx.realWorkDir();
    if (within(resolved, workDir) && resolved !== workDir && isFile(resolved)) {
      const workPath = relative(workDir, resolved).split(sep).join("/");
      return this.#put({ work_path: workPath, kind }, name);
    }
    const data = await readFile(path);
    return this.#put({ base64: data.toString("base64"), kind }, name || "agent/image");
  }

  /** A `data:` URL, decoded and stored. */
  #dataUrl(url: string): Promise<{ file: string }> {
    const comma = url.indexOf(",");
    const header = comma < 0 ? url : url.slice(0, comma);
    const encoded = comma < 0 ? "" : url.slice(comma + 1);
    const kind = (header.slice("data:".length).split(";", 1)[0] ?? "").toLowerCase() || "image/png";
    const data = Buffer.from(encoded, "base64");
    return this.#put({ base64: data.toString("base64"), kind }, "agent/image");
  }

  async #put(params: Record<string, unknown>, name: string): Promise<{ file: string }> {
    const ref = (await this.#ctx.channel.request("file.put", { ...params, name })) as FileRefWire;
    return { file: ref.digest };
  }
}

/** How a message names something given as a tool that is not one. */
function shownTool(item: unknown): string {
  if (typeof item === "function" && item.name) {
    return item.name;
  }
  return typeName(item);
}

/** The transcript an `agent.run` answer (or its error's `data`) carries; else none. */
function transcriptOf(data: unknown): JsonValue[] {
  if (isPlainObject(data) && Array.isArray(data.transcript)) {
    return [...(data.transcript as JsonValue[])];
  }
  return [];
}

/** Whether `path` is a file (following symbolic links). */
export function isFile(path: string): boolean {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

/** A capability's answer, read (`CallResult`). */
class Called implements CallResult {
  readonly key: string | null;
  readonly cached: boolean;
  readonly costUsd: number | null;
  readonly files: ReadonlyMap<string, InputFile>;
  readonly data: JsonValue;

  constructor(result: Record<string, unknown>) {
    this.key = typeof result.key === "string" ? result.key : null;
    this.cached = result.cached === true;
    this.costUsd = typeof result.cost_usd === "number" ? result.cost_usd : null;
    const files = isPlainObject(result.files) ? result.files : {};
    this.files = new Map(
      Object.entries(files).map(([name, ref]) => [name, new InputFile(ref as FileRefWire)]),
    );
    this.data = (result.data ?? null) as JsonValue;
  }

  get image(): InputFile {
    return this.#first("image");
  }

  get audio(): InputFile {
    return this.#first("audio");
  }

  get video(): InputFile {
    return this.#first("video");
  }

  get json(): unknown {
    const data = this.data;
    return isPlainObject(data) && Object.hasOwn(data, "json") ? data.json : data;
  }

  #first(family: string): InputFile {
    for (const file of this.files.values()) {
      if (familyOf(file.kind) === family) {
        return file;
      }
    }
    throw new CallFailed(`the call returned no ${family}`);
  }
}

function makeReader(ctx: RunContext): Reader {
  const one = (file: unknown): InputFile => {
    if (fileRefOf(file) !== undefined) {
      return file as InputFile;
    }
    if (typeof file !== "string") {
      throw new TypeError(`ctx.read takes an InputFile or an input's name, not ${typeName(file)}`);
    }
    if (!Object.hasOwn(ctx.inputs, file)) {
      throw new NodeFailure(`input ${file} is absent`);
    }
    const value = ctx.inputs[file];
    if (fileRefOf(value) === undefined) {
      throw new NodeFailure(`input ${file} is not one file`);
    }
    return value as InputFile;
  };
  const bytes = async (file: unknown): Promise<Uint8Array> =>
    readFile((fileRefOf(one(file)) as FileRefWire).path);
  return {
    bytes,
    text: async (file) => decodeText(await bytes(file)),
    json: async (file) => JSON.parse(decodeText(await bytes(file))) as unknown,
  };
}

function makeMaker(ctx: RunContext): Maker {
  const bytesOf = (data: unknown, what: string): Uint8Array => {
    if (!(data instanceof Uint8Array)) {
      throw new TypeError(`${what} takes bytes (a Uint8Array), not ${typeName(data)}`);
    }
    return new Uint8Array(data);
  };
  return {
    png(data) {
      const bytes = bytesOf(data, "ctx.out.png");
      if (!PNG_SIGNATURE.every((byte, index) => bytes[index] === byte)) {
        throw new TypeError("ctx.out.png takes an encoded PNG, and these bytes do not start like one");
      }
      return makeOutput("image/png", { data: bytes });
    },
    bytes(data, kind) {
      if (typeof kind !== "string" || !kind) {
        throw new TypeError("ctx.out.bytes needs the file's kind");
      }
      return makeOutput(kind, { data: bytesOf(data, "ctx.out.bytes") });
    },
    text(text, kind = "text/plain") {
      if (typeof text !== "string") {
        throw new TypeError(`ctx.out.text takes a string, not ${typeName(text)}`);
      }
      if (/\p{Cs}/u.test(text)) {
        throw new TypeError(
          "ctx.out.text takes a string without lone surrogates, which UTF-8 cannot hold",
        );
      }
      return makeOutput(kind, { data: new TextEncoder().encode(text) });
    },
    json(value) {
      try {
        checkValue(value);
      } catch (error) {
        throw new TypeError(`ctx.out.json takes a JSON value: ${(error as Error).message}`);
      }
      return makeOutput("json", { json: structuredClone(value) as JsonValue });
    },
    path(name) {
      return ctx.workPath(`out/${name}`);
    },
    file(path, kind) {
      const given = String(path);
      const resolved = realPath(resolve(given));
      const workDir = ctx.realWorkDir();
      const made = kind ?? kindOf(given);
      if (within(resolved, workDir) && resolved !== workDir) {
        if (!isFile(resolved)) {
          throw new Error(`${given} is not a file`);
        }
        return makeOutput(made, { workPath: relative(workDir, resolved).split(sep).join("/") });
      }
      return makeOutput(made, { data: new Uint8Array(readFileSync(given)) });
    },
  };
}

/** The longest delay a timer takes (Node runs a longer one at once). */
const LONGEST_TIMER_MS = 2 ** 31 - 1;

/** `ctx.tool(name)` (`ToolHandle`). */
class Program implements ToolHandle {
  readonly name: string;
  readonly executable: string;
  readonly workDir: string;
  readonly #ctx: RunContext;

  constructor(name: string, executable: string, workDir: string, ctx: RunContext) {
    this.name = name;
    this.executable = executable;
    this.workDir = workDir;
    this.#ctx = ctx;
  }

  run(argv: readonly string[], options: ToolRunOptions = {}): Promise<ToolRun> {
    const { cwd, timeoutS, env, check = true } = options;
    if (
      timeoutS !== undefined &&
      timeoutS !== null &&
      (typeof timeoutS !== "number" || !Number.isFinite(timeoutS) || timeoutS <= 0)
    ) {
      return Promise.reject(
        new TypeError(`timeoutS is a number of seconds above 0, or null, not ${pyRepr(timeoutS)}`),
      );
    }
    const signal = this.#ctx.signal;
    if (signal.aborted) {
      return Promise.reject(signal.reason);
    }
    return new Promise<ToolRun>((done, fail) => {
      const child = spawn(this.executable, [...argv], {
        cwd: cwd ?? this.workDir,
        env: env === undefined ? process.env : { ...env },
        stdio: ["ignore", "pipe", "pipe"],
        // A session of its own: a timeout ends everything it started.
        detached: true,
      });
      const group = child.pid;
      if (group !== undefined) {
        this.#ctx.startedTool(group);
      }
      const stdout: Buffer[] = [];
      const stderr: Buffer[] = [];
      child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
      child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
      let ended: "timeout" | "aborted" | null = null;
      let timer: NodeJS.Timeout | undefined;
      if (typeof timeoutS === "number") {
        // A timer longer than Node's longest is armed again in parts.
        const deadline = Date.now() + timeoutS * 1000;
        const arm = () => {
          const left = deadline - Date.now();
          timer = setTimeout(
            () => {
              if (left > LONGEST_TIMER_MS) {
                arm();
                return;
              }
              ended = "timeout";
              endGroup(group);
            },
            Math.max(0, Math.min(left, LONGEST_TIMER_MS)),
          );
        };
        arm();
      }
      const onAbort = () => {
        ended ??= "aborted";
        endGroup(group);
      };
      signal.addEventListener("abort", onAbort, { once: true });
      const finish = () => {
        if (timer !== undefined) {
          clearTimeout(timer);
        }
        signal.removeEventListener("abort", onAbort);
        if (group !== undefined) {
          this.#ctx.endedTool(group);
        }
      };
      child.once("error", (error) => {
        finish();
        fail(error);
      });
      child.once("close", (code, closedBy) => {
        finish();
        if (ended === "aborted") {
          fail(signal.reason);
          return;
        }
        if (ended === "timeout") {
          fail(new NodeFailure(`${this.name} ran past ${timeoutS} seconds`));
          return;
        }
        const status = code ?? -((closedBy === null ? 0 : constants.signals[closedBy]) ?? 0);
        const ran: ToolRun = {
          status,
          stdout: Buffer.concat(stdout).toString("utf8"),
          stderr: Buffer.concat(stderr).toString("utf8"),
        };
        if (check && status !== 0) {
          const tail = (ran.stderr || ran.stdout).trim().slice(-2000);
          fail(new NodeFailure(`${this.name} exited ${status}: ${tail}`));
          return;
        }
        done(ran);
      });
    });
  }
}

/** Ends a program started in a session of its own, with everything it started. */
function endGroup(pid: number | undefined): void {
  if (pid === undefined) {
    return;
  }
  try {
    process.kill(-pid, "SIGKILL");
  } catch {
    try {
      process.kill(pid, "SIGKILL");
    } catch {
      // already gone
    }
  }
}

/** `target` with its symbolic links resolved as far as it exists (as Python's `Path.resolve`). */
export function realPath(target: string): string {
  const resolved = resolve(target);
  const rest: string[] = [];
  let current = resolved;
  for (;;) {
    try {
      return rest.length === 0 ? realpathSync(current) : join(realpathSync(current), ...rest);
    } catch {
      const parent = dirname(current);
      if (parent === current) {
        return resolved;
      }
      rest.unshift(basename(current));
      current = parent;
    }
  }
}

/** Whether `path` is `folder` or lies under it (both absolute and resolved). */
export function within(path: string, folder: string): boolean {
  if (path === folder) {
    return true;
  }
  const rel = relative(folder, path);
  return rel !== "" && rel.split(sep)[0] !== ".." && !isAbsolute(rel);
}
