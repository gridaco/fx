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
import { isAbsolute, join, resolve } from "node:path";
import { exactRunTarget } from "./control.js";
import type {
  CallRecovery,
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
  RunEvent,
  RunFinished,
} from "./documents.js";
import { type CallOptions, call, commandDirectory, document, type Exit, failure, INTERRUPTED } from "./engine.js";
import { FxError } from "./errors.js";
import {
  amountText,
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
  /** The command's exit status. A duplicate rerun can return 0 for its accepted request.
   * Read `ok` and `incomplete` for the selected attempt's recorded outcome. */
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
    return this.paused ? undefined : this.currentEvent(this.projection.run?.run_finished);
  }

  private get pauseEvent(): Projection["run"]["run_paused"] {
    return this.currentEvent(this.projection.run?.run_paused);
  }

  private currentEvent<E extends RunEvent>(event: E | undefined): E | undefined {
    return (event?.invocation_id ?? null) === this.invocationId ? event : undefined;
  }

  /** The initialized invocation represented by this result's projection. */
  get invocationId(): string | null {
    const value = this.projection.run?.run_started?.invocation_id;
    return typeof value === "string" ? value : null;
  }

  /** The stable identity of this run, when available. */
  get runId(): string | null {
    return this.inspection.run?.run_id ?? null;
  }

  /** This result's attempt number within the run, when available. */
  get attemptNumber(): number | null {
    return this.inspection.run?.attempt_number ?? null;
  }

  /** Whether every step that had to run succeeded and the run ended normally. */
  get ok(): boolean {
    return this.finished?.ok === true;
  }

  /** Whether the attempt stopped before everything ran. */
  get incomplete(): boolean {
    const finished = this.finished;
    return finished === undefined ? true : finished.incomplete === true;
  }

  /** Whether the invocation recorded a durable pause after draining active work. */
  get paused(): boolean {
    return this.pauseEvent !== undefined;
  }

  /** Budget booked in US dollars over every invocation; unreported charges consume their reservations. */
  get chargedUsd(): number {
    return Number(this.inspection.run?.charged_usd ?? this.finished?.charged_usd ?? 0);
  }

  /** Recorded provider recovery checks and results, in event order across invocations. */
  get callRecoveries(): readonly CallRecovery[] {
    return this.inspection.run?.call_recoveries ?? [];
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
      for (const [name, value] of Object.entries((this.pauseEvent ?? this.finished)?.outputs ?? {})) {
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
  "waitForAnswers",
  "runDir",
  "name",
  "resume",
  "recover",
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
  const resuming = options.recover === true || options.resume !== undefined ||
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
      const given = {
        ...callOptions(request, signal),
        env: { ...request.env, GRIDA_FX_SDK_TEMP_INPUTS: inputsFile === null ? "0" : "1" },
      };
      return answerer === null ? call(args, given) : callAnswered(args, given, answerer);
    }),
  );
  const fault = answerer?.fault;
  if (fault !== null && fault !== undefined) {
    throw fault.error;
  }
  const folder = startedRunFolder("run", done);
  if (folder === undefined) {
    if (resuming) throw failure("run", done);
    const replanned = await planRequest(request, signal, models);
    if (!replanned.ok) {
      throw new PlanRefused(replanned);
    }
    throw failure("run", done);
  }
  return readRunResult(folder, done, callOptions(request, signal));
}

/** Execution options for continuing an exact saved attempt. */
export type ResumeOptions = Pick<
  RunOptions,
  "cwd" | "env" | "signal" | "live" | "maxUsd" | "yesUpTo" | "waitForAnswers" | "requireEstimates"
>;

/** Options for starting a fresh attempt of a saved run. */
export interface RerunOptions extends ResumeOptions {
  /** Use this attempt as the source. Defaults to the selected record or the latest attempt. */
  readonly attempt?: number;
  /** Keep this ID when retrying the same start request. */
  readonly requestId?: string;
}

/** Continue an exact saved attempt using the execution context recorded by the engine.
 * If the record needs explicit context, use `run(target, { resume, ... })` or its `runDir` form. */
export async function resume(run: string, options: ResumeOptions = {}): Promise<RunResult> {
  return executeSavedRun("resume", run, [], options);
}

/** Start a fresh attempt of a saved run, using the engine's verified execution context. */
export async function rerun(run: string, options: RerunOptions = {}): Promise<RunResult> {
  const { attempt, requestId, ...execution } = options;
  const selection = attemptArgs(attempt);
  if (requestId !== undefined) {
    if (typeof requestId !== "string" || requestId === "" || requestId.includes("\0")) {
      throw new TypeError("requestId is a nonempty string without NUL");
    }
    selection.push(`--request-id=${requestId}`);
  }
  return executeSavedRun("rerun", run, selection, execution);
}

async function executeSavedRun(
  verb: "resume" | "rerun",
  run: string,
  selection: string[],
  options: ResumeOptions,
): Promise<RunResult> {
  const { maxUsd, requireEstimates, ...execution } = options;
  if (requireEstimates !== undefined && typeof requireEstimates !== "boolean") {
    throw new TypeError("requireEstimates is true or false");
  }
  const args = [verb, exactRunTarget(run), ...selection, ...runArgs(execution), "--no-view"];
  if (maxUsd !== undefined) args.push(`--max-usd=${amountText("maxUsd", maxUsd)}`);
  if (requireEstimates) args.push("--require-estimates");
  const output = await call(args, options);
  const done = { ...output, stderr: output.stderr.replace(/^resume_project\t[^\r\n]*(?:\r?\n|$)/gm, "") };
  const folder = startedRunFolder(verb, done);
  if (folder === undefined) throw failure(verb, done);
  if (!isAbsolute(folder)) {
    throw new FxError(`grida-fx ${verb} did not report the absolute run folder`, { exitCode: done.status });
  }
  return readRunResult(folder, done, options);
}

function startedRunFolder(verb: "run" | "resume" | "rerun", done: Exit): string | undefined {
  if (done.status === INTERRUPTED) {
    throw new FxError("the run was interrupted", {
      exitCode: done.status,
      stdout: done.stdout,
      stderr: done.stderr,
      args: done.args,
    });
  }
  if (done.status !== 0 && done.status !== 1) {
    throw failure(verb, done);
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
  }
  return folder;
}

async function readRunResult(folder: string, done: Exit, options: CallOptions): Promise<RunResult> {
  const runDir = isAbsolute(folder) ? folder : resolve(commandDirectory(options.cwd), folder);
  const projection = await project(runDir, options);
  const inspection = await inspect(runDir, options);
  return new RunResult(runDir, done, projection, inspection);
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
  /** Pin an exact attempt of the selected run. */
  readonly attempt?: number;
  /** Re-check every placed file against its record (`--verify`). */
  readonly verify?: boolean;
}

export function attemptArgs(attempt: number | undefined): string[] {
  if (attempt === undefined) return [];
  if (!Number.isSafeInteger(attempt) || attempt < 1) {
    throw new TypeError("attempt is a positive safe integer");
  }
  return [`--attempt=${attempt}`];
}

/** `grida-fx inspect <run> --json`: a run's summary. Pass `attempt` to read a specific attempt.
 * With `verify`, `verification` says whether every placed file still matches. */
export async function inspect(run: string, options: InspectOptions = {}): Promise<Inspection> {
  const args = [run, "--json", ...attemptArgs(options.attempt), ...(options.verify === true ? ["--verify"] : [])];
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
