/**
 * Planning and running from JavaScript, by driving the `grida-fx` binary: the engine is the only
 * place with engine logic, and these functions read the documents it prints.
 *
 * - A target is what the command takes: a workflow file, a workflow id, or a builder
 *   `file.ts:function` or `file.py:function` (with `args`). `run` also takes a {@link Plan}: it
 *   runs it with the target and options it was planned with.
 * - Relative paths (`inputFiles`, `routes`, `runDir`, paths inside `inputs`) start at `cwd`, as on
 *   the command line.
 * - Planning never spends. A run spends only with `live: true`, within a ceiling; with
 *   `standIn`, a function answers its paid calls instead, offline and for nothing.
 * - The engine's errors (exit status 2), a run it refuses to start and an interrupted run reject
 *   with {@link FxError}; a plan with problems that is asked to run rejects with
 *   {@link PlanRefused}. A failed step does not reject: read `result.ok` and `result.failed`.
 */

import { lstat, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import type {
  Encoded,
  FileRef,
  Graph,
  Identities,
  Inspection,
  Price,
  PricePhase,
  Problem,
  ProjectedInstance,
  Projection,
  RunFinished,
} from "./documents.js";
import { type CallOptions, call, document, type Exit, failure, INTERRUPTED } from "./engine.js";
import { FxError } from "./errors.js";
import {
  makeRequest,
  type PlanOptions,
  type PlanRequest,
  planningArgs,
  type RunOptions,
  runArgs,
  withInputsFile,
} from "./request.js";
import { socketPair } from "./socket.js";
import { Answerer, serveStandIn } from "./stand-in.js";
import type { JsonValue } from "./workflow.js";

/** The first line of a run's summary: `run` padded to 10 columns, then the folder as named. */
const RUN_LINE = "run       ";
/** How the engine says it would not start a run (on stdout, exit status 1). */
const REFUSED = "refused: ";
/** Serialized privately so changing the public graph cannot change prepared model facts. */
const resolvedModels = new WeakMap<Plan, string>();

// ------------------------------------------------------------------------------------------------
// Plans

/** A plan the engine made: the expanded graph and its price. */
export class Plan {
  /** The expanded graph (fx-graph-v1 or fx-graph-v2), as `grida-fx plan --json` prints it. */
  readonly graph: Graph;
  /** The price by phase, as `grida-fx price` prints it. */
  readonly price: Price;
  /** The target and options it was planned with; `run(plan)` uses them. */
  readonly request: PlanRequest;

  constructor(graph: Graph, price: Price, request: PlanRequest) {
    this.graph = graph;
    this.price = price;
    this.request = request;
  }

  /** Whether the plan has no problems, so it may run. */
  get ok(): boolean {
    return this.problems.length === 0;
  }

  /** Why the plan is refused; empty when it is not. */
  get problems(): Problem[] {
    return (this.graph.problems ?? []).map((problem) => ({
      where: String(problem.where ?? ""),
      message: String(problem.message ?? ""),
    }));
  }

  /** The total forecast; null amounts mean it includes unpriced calls. */
  get estimate(): { lowUsd: number | null; highUsd: number | null; ceilingUsd: number | null } {
    const estimate = this.price.estimate ?? this.graph.estimate;
    return {
      lowUsd: estimate.low_usd,
      highUsd: estimate.high_usd,
      ceilingUsd: this.price.ceiling_usd ?? this.graph.estimate.ceiling_usd,
    };
  }

  /** Forecast coverage and the known subtotal; allowances never stand in for a bill. */
  get forecast(): { lowUsd: number | null; highUsd: number | null; knownLowUsd: number | null; knownHighUsd: number | null; unpricedCalls: number } {
    const estimate = this.price.estimate ?? this.graph.estimate;
    return {
      lowUsd: estimate.low_usd,
      highUsd: estimate.high_usd,
      knownLowUsd: estimate.known_low_usd ?? estimate.low_usd,
      knownHighUsd: estimate.known_high_usd ?? estimate.high_usd,
      unpricedCalls: estimate.unpriced_calls ?? 0,
    };
  }

  /** The price by phase. */
  get phases(): PricePhase[] {
    return this.price.phases ?? [];
  }

  /** The workflow's id. */
  get workflowId(): string {
    return this.graph.workflow?.id ?? "";
  }
}

/** A plan with problems was asked to run: `the plan is refused:` and one line per problem. */
export class PlanRefused extends FxError {
  /** The refused plan. */
  readonly plan: Plan;

  constructor(plan: Plan) {
    const lines = plan.problems.map((problem) => `${problem.where}: ${problem.message}`);
    super(["the plan is refused:", ...lines].join("\n"));
    this.name = "PlanRefused";
    this.plan = plan;
  }
}

/** `grida-fx expand`: the expanded graph (fx-graph-v1 or fx-graph-v2), with `problems`. Never spends. */
export async function expand(target: string, options: PlanOptions = {}): Promise<Graph> {
  return planningDocument<Graph>("expand", makeRequest(target, options), options.signal);
}

/** `grida-fx identity`: each live instance's identity (null while it waits on a result). */
export async function identity(target: string, options: PlanOptions = {}): Promise<Identities> {
  return planningDocument<Identities>("identity", makeRequest(target, options), options.signal);
}

/** `grida-fx price`: the plan's price by phase. Never spends. */
export async function price(target: string, options: PlanOptions = {}): Promise<Price> {
  return planningDocument<Price>("price", makeRequest(target, options), options.signal);
}

/** Expands, checks and prices `target` in one engine invocation. Never spends;
 * `plan.problems` says what is wrong. */
export async function plan(target: string, options: PlanOptions = {}): Promise<Plan> {
  return planRequest(makeRequest(target, options), options.signal);
}

async function planRequest(
  request: PlanRequest,
  signal: AbortSignal | undefined,
  models: string | undefined = undefined,
): Promise<Plan> {
  const prepared = await withResolvedModels(models, (modelsFile) =>
    planningDocument<Record<string, unknown>>("plan", request, signal, [
      "--json",
      "--prepared",
      ...(modelsFile === null ? [] : [`--resolved-models=${modelsFile}`]),
    ]),
  );
  const object = (value: unknown): value is Record<string, unknown> =>
    value !== null && typeof value === "object" && !Array.isArray(value);
  const amount = (value: unknown): value is number =>
    typeof value === "number" && Number.isFinite(value) && value >= 0;
  const range = (value: unknown): boolean => object(value) && (
    amount(value.low_usd) && amount(value.high_usd) && value.low_usd <= value.high_usd
      && value.unpriced_calls === undefined && value.known_low_usd === undefined && value.known_high_usd === undefined
    || value.low_usd === null && value.high_usd === null
      && amount(value.known_low_usd) && amount(value.known_high_usd) && value.known_low_usd <= value.known_high_usd
      && typeof value.unpriced_calls === "number" && Number.isSafeInteger(value.unpriced_calls) && value.unpriced_calls > 0
  );
  const ceiling = (value: unknown): boolean => value === null || amount(value);
  if (
    prepared.kind !== "fx-prepared-plan-v1" ||
    !object(prepared.graph) || (prepared.graph.kind !== "fx-graph-v1" && prepared.graph.kind !== "fx-graph-v2") ||
    !object(prepared.graph.workflow) || typeof prepared.graph.workflow.id !== "string" ||
    typeof prepared.graph.workflow.title !== "string" ||
    !Array.isArray(prepared.graph.instances) || !Array.isArray(prepared.graph.pending) ||
    !Array.isArray(prepared.graph.problems) || !prepared.graph.problems.every((problem) =>
      object(problem) && typeof problem.where === "string" && typeof problem.message === "string") ||
    !object(prepared.graph.estimate) || !range(prepared.graph.estimate) ||
    prepared.graph.kind === "fx-graph-v1" && prepared.graph.estimate.low_usd === null ||
    !ceiling(prepared.graph.estimate.ceiling_usd) ||
    !object(prepared.price) || !range(prepared.price.estimate) ||
    object(prepared.price.estimate) && prepared.price.estimate.low_usd === null && prepared.price.kind !== "fx-price-v2" ||
    !ceiling(prepared.price.ceiling_usd) || !Array.isArray(prepared.price.phases) ||
    !prepared.price.phases.every(range) || !object(prepared.resolved_models) ||
    prepared.resolved_models.fx !== "routes/v1" || !Array.isArray(prepared.resolved_models.routes)
  ) {
    throw new FxError("grida-fx plan returned an invalid prepared plan");
  }
  const planned = new Plan(
    prepared.graph as unknown as Graph,
    prepared.price as unknown as Price,
    request,
  );
  resolvedModels.set(planned, JSON.stringify(prepared.resolved_models));
  return planned;
}

async function mayExist(path: string): Promise<boolean> {
  try {
    await lstat(path);
    return true;
  } catch (error) {
    // The engine owns diagnostics for an inaccessible or otherwise invalid existing path.
    return (error as NodeJS.ErrnoException).code !== "ENOENT";
  }
}

async function withResolvedModels<T>(
  models: string | undefined,
  work: (modelsFile: string | null) => Promise<T>,
): Promise<T> {
  if (models === undefined) return work(null);
  // Model facts contain no relative file references; keep them off the source tree.
  const directory = await mkdtemp(join(tmpdir(), "grida-fx-models-"));
  const path = join(directory, "models.json");
  try {
    await writeFile(path, models, { encoding: "utf8", mode: 0o600, flag: "wx" });
    return await work(path);
  } finally {
    await rm(directory, { recursive: true, force: true }).catch(() => undefined);
  }
}

async function planningDocument<T>(
  verb: string,
  request: PlanRequest,
  signal: AbortSignal | undefined,
  extra: readonly string[] = [],
): Promise<T> {
  return withInputsFile(request, (inputsFile) =>
    document<T>(
      verb,
      [...planningArgs(request, inputsFile), ...extra],
      callOptions(request, signal),
    ),
  );
}

function callOptions(request: PlanRequest, signal: AbortSignal | undefined): CallOptions {
  return {
    cwd: request.cwd,
    ...(request.env !== undefined ? { env: request.env } : {}),
    ...(signal !== undefined ? { signal } : {}),
  };
}

// ------------------------------------------------------------------------------------------------
// Runs

/** A file a run made, by digest. */
export class RunFile {
  readonly digest: string;
  /** The file kind (`image/png`, `json`, …). */
  readonly kind: string;
  readonly name: string;
  readonly size: number;
  /** A collection item's key, if it has one. */
  readonly key: string | null;
  /** Where the run placed it (an absolute path in the run folder), or null when no step of this
   * run placed it (a workflow output that is one of its input files). Read it; never write it:
   * it shares its bytes with the cache. */
  readonly path: string | null;

  constructor(ref: FileRef, path: string | null) {
    if (typeof ref?.digest !== "string" || !/^[0-9a-f]{64}$/.test(ref.digest)) {
      throw new FxError(
        `the run's record names a file by ${JSON.stringify(ref?.digest)}, which is not a digest`,
      );
    }
    this.digest = ref.digest;
    this.kind = ref.kind ?? "file";
    this.name = ref.name ?? "";
    this.size = ref.size ?? 0;
    this.key = ref.key ?? null;
    this.path = path;
  }

  /** The file's bytes. */
  async read(): Promise<Buffer> {
    if (this.path === null) {
      throw new FxError(`the run placed no copy of ${this.name || this.digest} in its folder`);
    }
    return readFile(this.path);
  }
}

/** A value of a run: a file, a keyed collection (in its order), a list, a plain value, or null. */
export type RunValue = RunFile | Map<string, RunValue> | RunValue[] | JsonValue;

/** A finished `run`: read from the engine's `project` and `inspect --json` of its folder. */
export class RunResult {
  /** The run folder, absolute. */
  readonly runDir: string;
  /** The engine's exit status: 0 when the run is ok (and every `deliver` found its output). */
  readonly exitCode: number;
  /** What `grida-fx run` printed: the plan and the run's summary. */
  readonly stdout: string;
  /** The run's record, projected (`grida-fx project`). */
  readonly projection: Projection;
  /** The run's summary (`grida-fx inspect --json`). */
  readonly inspection: Inspection;
  #outputs: Record<string, RunValue> | undefined;

  constructor(runDir: string, exit: Exit, projection: Projection, inspection: Inspection) {
    this.runDir = runDir;
    this.exitCode = exit.status ?? -1;
    this.stdout = exit.stdout;
    this.projection = projection;
    this.inspection = inspection;
  }

  private get finished(): RunFinished | undefined {
    return this.projection.run?.run_finished;
  }

  /** The initialized invocation represented by this result's projection. */
  get invocationId(): string | null {
    const value = this.projection.run?.run_started?.invocation_id;
    return typeof value === "string" ? value : null;
  }

  /** Whether every step that had to run succeeded and the run ended normally. */
  get ok(): boolean {
    return this.finished?.ok === true;
  }

  /** Whether the run stopped before everything ran (running it again continues it). */
  get incomplete(): boolean {
    const finished = this.finished;
    return finished === undefined ? true : finished.incomplete === true;
  }

  /** Budget booked in US dollars over every invocation; unreported charges consume their reservations. */
  get chargedUsd(): number {
    return Number(this.inspection.run?.charged_usd ?? this.finished?.charged_usd ?? 0);
  }

  /** The ids of the instances that failed. */
  get failed(): string[] {
    return (this.finished?.failed ?? []).map(String);
  }

  /** Why the run stopped early, if it did. */
  get stopped(): string | null {
    return this.finished?.stopped ?? null;
  }

  /** Whether the run was a stand-in run (`run(…, { standIn })`, or `--stand-in`). */
  get standIn(): boolean {
    return this.projection.run?.run_started?.stand_in === true;
  }

  /** Each instance's state, by instance id. */
  get steps(): Record<string, ProjectedInstance> {
    return this.projection.instances ?? {};
  }

  /** Each declared output: a {@link RunFile}, a `Map` for a keyed collection, an array, a plain
   * value, or null. */
  get outputs(): Record<string, RunValue> {
    if (this.#outputs === undefined) {
      const placed = new Map<string, string>();
      for (const step of this.inspection.run?.steps ?? []) {
        for (const file of step.files ?? []) {
          if (!placed.has(file.digest)) {
            placed.set(file.digest, resolve(this.runDir, file.path));
          }
        }
      }
      const outputs: Record<string, RunValue> = {};
      for (const [name, value] of Object.entries(this.finished?.outputs ?? {})) {
        outputs[name] = decode(value, placed);
      }
      this.#outputs = outputs;
    }
    return this.#outputs;
  }
}

/** A value of the run's record (fx-run-events-v1 `encoded`). */
export function decode(encoded: Encoded, placed: ReadonlyMap<string, string>): RunValue {
  if (typeof encoded !== "object" || encoded === null || Object.keys(encoded).length !== 1) {
    throw new FxError(
      `the run's record holds a value in no known form: ${JSON.stringify(encoded)}`,
    );
  }
  if ("file" in encoded) {
    return new RunFile(encoded.file, placed.get(encoded.file?.digest) ?? null);
  }
  if ("collection" in encoded) {
    return new Map(encoded.collection.map(([key, item]) => [String(key), decode(item, placed)]));
  }
  if ("list" in encoded) {
    return encoded.list.map((item) => decode(item, placed));
  }
  if ("none" in encoded) {
    return null;
  }
  if ("value" in encoded) {
    return encoded.value;
  }
  throw new FxError(`the run's record holds a value in no known form: ${JSON.stringify(encoded)}`);
}

/** The options `run` still takes with a {@link Plan}. */
const RUN_ONLY: readonly string[] = [
  "live",
  "yesUpTo",
  "runDir",
  "name",
  "resume",
  "deliver",
  "signal",
  "standIn",
];

/**
 * Starts a new run of `target`, or a plan, after planning it: a plan with problems rejects with
 * {@link PlanRefused} before any folder exists. `live: true` admits paid calls, within a
 * ceiling. Then reads the folder back through `grida-fx project` and `inspect --json`.
 * A named resume or existing explicit folder is planned directly by the engine using its
 * recorded model facts; an engine refusal rejects with {@link FxError}.
 *
 * With `standIn`, the function answers the run's paid calls (`spec/protocol.md` §5.7): it is
 * served in this process, over a socket that is the engine's standard input, and the run is not
 * planned first (the engine plans it, and refuses a plan with problems). When the stand-in fails
 * (it throws something other than `CallRefused` or `CallFailed`, or returns something that is not
 * an answer), the engine stops the run, and `run` rejects with what the stand-in threw (a
 * `TypeError` for what it returned) once the engine has ended.
 */
export async function run(target: string | Plan, options: RunOptions = {}): Promise<RunResult> {
  let request: PlanRequest;
  if (target instanceof Plan) {
    const given = Object.entries(options)
      .filter(([name, value]) => value !== undefined && !RUN_ONLY.includes(name))
      .map(([name]) => name);
    if (given.length > 0) {
      throw new TypeError(
        `a Plan runs with the target and options it was planned with, so it takes no ` +
          `${given.join(", ")}: plan again to change them`,
      );
    }
    request = target.request;
  } else {
    request = makeRequest(target, options);
  }
  const own = runArgs(options);
  const signal = options.signal;
  const answerer = options.standIn === undefined ? null : new Answerer(options.standIn);
  const resuming = options.resume !== undefined ||
    (options.runDir !== undefined && await mayExist(resolve(request.cwd, options.runDir)));
  // The CLI loads recorded model facts before planning an existing run.
  const planned = resuming ? null
    : target instanceof Plan ? target : answerer === null ? await planRequest(request, signal) : null;
  if (planned !== null && !planned.ok) {
    throw new PlanRefused(planned);
  }
  const models = planned === null ? undefined
    : resolvedModels.get(planned) ?? '{"fx":"routes/v1","routes":[]}';
  const done = await withResolvedModels(models, (modelsFile) =>
    withInputsFile(request, (inputsFile) => {
      const args = [
        "run", ...planningArgs(request, inputsFile), ...own, "--no-view",
        ...(modelsFile === null ? [] : [`--resolved-models=${modelsFile}`]),
      ];
      const given = callOptions(request, signal);
      return answerer === null ? call(args, given) : callAnswered(args, given, answerer);
    }),
  );
  const fault = answerer?.fault;
  if (fault !== null && fault !== undefined) {
    throw fault.error;
  }
  if (done.status === INTERRUPTED) {
    throw new FxError("the run was interrupted", {
      exitCode: done.status,
      stdout: done.stdout,
      stderr: done.stderr,
      args: done.args,
    });
  }
  if (done.status !== 0 && done.status !== 1) {
    throw failure("run", done);
  }
  const lines = done.stdout.split("\n");
  const named = lines
    .filter((line) => line.startsWith(RUN_LINE))
    .map((line) => line.slice(RUN_LINE.length));
  const folder = named.at(-1);
  if (folder === undefined) {
    const refused = lines.filter((line) => line.startsWith(REFUSED));
    if (refused.length > 0) {
      throw new FxError(refused.at(-1) ?? "refused", {
        exitCode: done.status,
        stdout: done.stdout,
        stderr: done.stderr,
        args: done.args,
      });
    }
    if (resuming) throw failure("run", done);
    const replanned = await planRequest(request, signal, models);
    if (!replanned.ok) {
      throw new PlanRefused(replanned);
    }
    throw failure("run", done);
  }
  const readOptions = callOptions(request, signal);
  const projection = await project(folder, readOptions);
  const inspection = await inspect(folder, readOptions);
  return new RunResult(resolve(request.cwd, folder), done, projection, inspection);
}

/** {@link call} with the stand-in served on a socket pair, the engine's end its standard input.
 * Once the engine has exited, the serving ends: nothing waits for a stand-in still answering. */
async function callAnswered(
  args: readonly string[],
  options: CallOptions,
  answerer: Answerer,
): Promise<Exit> {
  const { ours, theirs } = await socketPair();
  const served = serveStandIn(ours, answerer);
  try {
    return await call(args, options, theirs);
  } finally {
    theirs.destroy();
    answerer.close();
    ours.destroy();
    await served;
  }
}

/** `grida-fx project <run>`: a run's record projected to its state. */
export async function project(runDir: string, options: CallOptions = {}): Promise<Projection> {
  return document<Projection>("project", [runDir], options, [0]);
}

/** Options of {@link inspect}. */
export interface InspectOptions extends CallOptions {
  /** Re-check every placed file against its record (`--verify`). */
  readonly verify?: boolean;
}

/** `grida-fx inspect <run> --json`: a run's summary. `run` is a run folder, or a workflow id for
 * its newest run. With `verify`, `verification` says whether every placed file still matches. */
export async function inspect(run: string, options: InspectOptions = {}): Promise<Inspection> {
  const args = [run, "--json", ...(options.verify === true ? ["--verify"] : [])];
  return document<Inspection>("inspect", args, options, [0, 1]);
}

/** The engine's version (`grida-fx --version`), e.g. `0.1.0-alpha.1`. */
export async function engineVersion(options: CallOptions = {}): Promise<string> {
  const done = await call(["--version"], options);
  if (done.status !== 0) {
    throw failure("--version", done);
  }
  return done.stdout.trim().replace(/^grida-fx\s+/, "");
}
