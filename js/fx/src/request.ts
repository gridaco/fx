/**
 * The command line of a request: a target and its options as the engine's planning verbs and
 * `run` take them (`docs/guide/05-running.md`). Every option goes as `--name=value`, so a value
 * that looks like a flag stays a value.
 */

import { randomBytes } from "node:crypto";
import { unlink, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import type { Environment } from "./binary.js";
import type { CallOptions } from "./engine.js";
import { toYaml } from "./yaml.js";

/** Options of the planning verbs (`plan`, `expand`, `identity`, `price`) and of `run`. */
export interface PlanOptions extends CallOptions {
  /** Input values by name. They are written to a temporary inputs file in `cwd` (so relative
   * paths in them keep their meaning) and given after `inputFiles`, so they win. */
  readonly inputs?: Readonly<Record<string, unknown>>;
  /** Inputs YAML files (`--inputs`), relative to `cwd`; later files win. */
  readonly inputFiles?: readonly string[];
  /** Route tables (`--routes`, `fx: routes/v1`); giving any leaves the built-in table out. */
  readonly routes?: readonly string[];
  /** Arguments of a builder target `file.py:function` (`--arg NAME=VALUE`). */
  readonly args?: Readonly<Record<string, string | number | boolean>>;
  /** The ceiling in US dollars (`--max-usd`); a live run needs one (here, in the workflow or in
   * fx.yaml). */
  readonly maxUsd?: number | string;
}

/** Options of `run`: the planning options, plus the run's own. */
export interface RunOptions extends PlanOptions {
  /** Admit paid provider calls (`--live`). Only `true` does; nothing turns it on by default. */
  readonly live?: boolean;
  /** Stop before a later phase that may take the run past this amount (`--yes-up-to`). */
  readonly yesUpTo?: number | string;
  /** The run folder, relative to `cwd` (`--run`); a folder holding a run of the same plan is
   * resumed. Default: a new folder under the project's runs folder. */
  readonly runDir?: string;
  /** Create a named run (`--name`); an existing name is refused by the engine. */
  readonly name?: string;
  /** Continue an existing named run (`--resume`) with the same target and inputs. */
  readonly resume?: string;
  /** Copy outputs out after the run (`--deliver OUTPUT=PATH`): `{ output: path }`, with `{key}`
   * in the path for each element of a collection. */
  readonly deliver?: Readonly<Record<string, string>>;
}

/** A target with its planning options, resolved. */
export interface PlanRequest {
  readonly target: string;
  readonly inputs: Readonly<Record<string, unknown>> | null;
  readonly inputFiles: readonly string[];
  readonly routes: readonly string[];
  readonly args: readonly (readonly [string, string])[];
  readonly maxUsd: string | null;
  /** Absolute. */
  readonly cwd: string;
  readonly env: Environment | undefined;
}

/** The request of `target` and `options`. */
export function makeRequest(target: string, options: PlanOptions): PlanRequest {
  if (typeof target !== "string" || target === "") {
    throw new TypeError(
      "a target is a workflow file, a workflow id or a builder file.py:function, as a string",
    );
  }
  if (target.startsWith("-")) {
    throw new TypeError(`a target cannot start with "-" (${target}): write it as ./${target}`);
  }
  const inputs = options.inputs ?? null;
  if (inputs !== null && (typeof inputs !== "object" || Array.isArray(inputs))) {
    throw new TypeError("inputs is an object of input names to values");
  }
  const args = Object.entries(options.args ?? {}).map(([name, value]): [string, string] => {
    if (!["string", "number", "boolean"].includes(typeof value)) {
      throw new TypeError(`args.${name} is a string, a number or a boolean`);
    }
    return [name, String(value)];
  });
  return {
    target,
    inputs: inputs !== null && Object.keys(inputs).length > 0 ? inputs : null,
    inputFiles: strings("inputFiles", options.inputFiles),
    routes: strings("routes", options.routes),
    args,
    maxUsd: options.maxUsd === undefined ? null : amountText("maxUsd", options.maxUsd),
    cwd: resolve(options.cwd ?? process.cwd()),
    env: options.env,
  };
}

/** The target and the planning options; `inputsFile` holds `request.inputs`. */
export function planningArgs(request: PlanRequest, inputsFile: string | null): string[] {
  const args = [request.target];
  for (const path of request.inputFiles) {
    args.push(`--inputs=${path}`);
  }
  if (inputsFile !== null) {
    args.push(`--inputs=${inputsFile}`);
  }
  for (const path of request.routes) {
    args.push(`--routes=${path}`);
  }
  for (const [name, value] of request.args) {
    args.push(`--arg=${name}=${value}`);
  }
  if (request.maxUsd !== null) {
    args.push(`--max-usd=${request.maxUsd}`);
  }
  return args;
}

/** The options of `run` alone. */
export function runArgs(options: RunOptions): string[] {
  const args: string[] = [];
  const selected = ["runDir", "name", "resume"] as const;
  if (selected.filter((name) => options[name] !== undefined).length > 1) {
    throw new TypeError("runDir, name and resume are mutually exclusive");
  }
  for (const name of selected) {
    const value = options[name];
    if (value !== undefined && (typeof value !== "string" || value === "" || value.includes("\0"))) {
      throw new TypeError(`${name} is a nonempty string without NUL`);
    }
  }
  if (options.live !== undefined && typeof options.live !== "boolean") {
    throw new TypeError(`live is true or false, not ${JSON.stringify(options.live)}`);
  }
  if (options.live === true) {
    args.push("--live");
  }
  if (options.yesUpTo !== undefined) {
    args.push(`--yes-up-to=${amountText("yesUpTo", options.yesUpTo)}`);
  }
  if (options.runDir !== undefined) {
    args.push(`--run=${options.runDir}`);
  }
  if (options.name !== undefined) args.push(`--name=${options.name}`);
  if (options.resume !== undefined) args.push(`--resume=${options.resume}`);
  for (const [name, path] of Object.entries(options.deliver ?? {})) {
    if (typeof path !== "string" || name === "" || path === "") {
      throw new TypeError(`deliver.${name} is a path`);
    }
    args.push(`--deliver=${name}=${path}`);
  }
  return args;
}

/**
 * An amount of US dollars as the command line takes it: decimal digits, never an exponent. A
 * string is passed as it is; the engine reads it with its money rules and refuses what they do
 * not allow.
 */
export function amountText(option: string, value: number | string | bigint): string {
  if (typeof value === "string" || typeof value === "bigint") {
    return String(value);
  }
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError(`${option} is an amount of US dollars, not ${String(value)}`);
  }
  const text = Object.is(value, -0) ? "0" : String(value);
  const exponent = /^(-?)([0-9]+)(?:\.([0-9]+))?e([-+][0-9]+)$/.exec(text);
  if (exponent === null) {
    return text;
  }
  const [, sign = "", whole = "", fraction = "", power = "0"] = exponent;
  const digits = whole + fraction;
  const point = whole.length + Number(power);
  let written: string;
  if (point <= 0) {
    written = `0.${"0".repeat(-point)}${digits}`;
  } else if (point >= digits.length) {
    written = digits + "0".repeat(point - digits.length);
  } else {
    written = `${digits.slice(0, point)}.${digits.slice(point)}`;
  }
  return sign + written.replace(/^0+(?=[0-9])/, "");
}

/** Runs `work` with `request.inputs` in a temporary inputs file in `cwd` (its name, relative);
 * the file is removed after. */
export async function withInputsFile<T>(
  request: PlanRequest,
  work: (inputsFile: string | null) => Promise<T>,
): Promise<T> {
  if (request.inputs === null) {
    return work(null);
  }
  const text = toYaml(request.inputs);
  const name = `.grida-fx-inputs-${randomBytes(8).toString("hex")}.yaml`;
  const path = join(request.cwd, name);
  await writeFile(path, text, { encoding: "utf8", flag: "wx" });
  try {
    return await work(name);
  } finally {
    await unlink(path).catch(() => undefined);
  }
}

function strings(option: string, values: readonly string[] | undefined): string[] {
  const list = [...(values ?? [])];
  for (const value of list) {
    if (typeof value !== "string" || value === "") {
      throw new TypeError(`${option} holds paths, as strings`);
    }
  }
  return list;
}
