/**
 * Workflow documents (`fx: workflow/v1`, `spec/schemas/fx-workflow-v1.schema.json`) as
 * TypeScript types, and a small builder. The engine gives the document its meaning and checks
 * it when it plans; these types only help write one. Write it out with `toYaml` and plan it:
 *
 * ```ts
 * import { writeFile } from "node:fs/promises";
 * import { expr, plan, toYaml, workflow } from "@grida/fx";
 *
 * const icon = workflow({
 *   id: "icon",
 *   title: "One item icon",
 *   inputs: { name: { type: "string", description: "What the item is" } },
 *   steps: {
 *     draw: {
 *       uses: "fx/image.generate@1",
 *       with: { prompt: `A single ${expr("inputs.name")} game icon`, background: "transparent" },
 *     },
 *   },
 *   outputs: { icon: expr("steps.draw.outputs.image") },
 * });
 * await writeFile("workflows/icon.yaml", toYaml(icon));
 * const planned = await plan("workflows/icon.yaml", { inputs: { name: "copper lantern" } });
 * ```
 */

/** A JSON value as FX holds it (`spec/identity.md` §1). */
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

/** An expression or a value that holds `${{ … }}` (kept as a string in the document). */
export type Expression = string;

/** One input declaration: a typed field, a `$ref` to another workflow's input, or a nested
 * object of fields. */
export type InputDeclaration = InputField | InputRef | { [field: string]: InputDeclaration };

/** A typed input field. */
export interface InputField {
  type: "string" | "integer" | "number" | "boolean" | "file" | "files" | "list" | "map";
  /** `file` and `files`: the file kind, or kind family, the input accepts. */
  kind?: string;
  /** `files`: the value is a glob pattern. */
  glob?: boolean;
  /** `list`: its items. */
  items?: InputDeclaration;
  /** `map`: its values. */
  values?: InputDeclaration;
  /** The input may be left out; it is then null. */
  optional?: boolean;
  description?: string;
  default?: JsonValue;
  enum?: JsonValue[];
  minimum?: number;
  maximum?: number;
  exclusive_minimum?: number;
  exclusive_maximum?: number;
  multiple_of?: number;
  min_length?: number;
  max_length?: number;
  pattern?: string;
  min_items?: number;
  max_items?: number;
  unique_items?: boolean;
  format?: string;
  examples?: JsonValue[];
}

/** Another workflow's input: `<file>#/<json pointer>`. */
export interface InputRef {
  $ref: string;
  optional?: boolean;
  default?: JsonValue;
  description?: string;
}

/** A spending ceiling. */
export interface Budget {
  max_usd: number;
}

/** A check made while planning, and again once what it reads exists. */
export interface Assertion {
  check: Expression | boolean;
  message: string;
  on_fail?: "fail" | "skip";
}

/** Which take to keep: the lowest or highest of a fact. */
export interface KeepBest {
  by: string;
  order: "lowest" | "highest";
}

/** Redo a judged step, or a group, as further takes. */
export interface Regeneration {
  /** Takes in all, the first included: 1 to 12, or an expression known while planning. */
  max: number | Expression;
  then?: "fail" | "continue" | "skip" | { keep_best: KeepBest };
  until?: Expression | null;
  feedback?: boolean;
}

/** Fields every step may have. */
interface StepCommon {
  if?: Expression | boolean | null;
  needs?: string[];
  /** Repeat over a list: an expression or a literal list. */
  for_each?: Expression | JsonValue[] | null;
  /** The name each repeated item takes (default `item`). */
  as?: string;
  key?: Expression | null;
  max?: number | Expression | null;
  matrix?: { [name: string]: Expression | JsonValue[] } | null;
  assert?: Assertion[];
  budget?: Budget | null;
  concurrency?: number | null;
  view?: boolean | string;
  timeout?: number | null;
  /** For readers; never part of what the step makes. */
  title?: string | null;
  description?: string | null;
}

/** A step that runs a node type. */
export interface NodeStep extends StepCommon {
  /** `fx/<name>@<major>` (built-in), `./<path>#<attr>` (a project node), or `./<path>.yaml`. */
  uses: string;
  /** The type's inputs and params; values holding `${{ }}` are expressions. */
  with?: { [name: string]: JsonValue };
  /** The step this judge gives a verdict on. */
  judges?: string | null;
  on_reject?: "fail" | "continue" | "skip" | { regenerate: Regeneration };
  takes?: number | null;
  pick?: "manual" | "first_accepted" | { best: KeepBest } | null;
  /** `plan`: a free, local, deterministic step that runs while planning. */
  at?: "plan" | null;
  /** The `model@provider` serving the type's one paid capability. */
  route?: string | null;
  /** Features the route must support. */
  requires?: string[];
  independent_of?: string | string[];
}

/** A group of steps, run together, maybe regenerated as a whole. */
export interface GroupStep extends StepCommon {
  steps: { [name: string]: Step };
  regenerate?: (Regeneration & { until: Expression }) | null;
}

/** One step: a node type (`uses`) or a group (`steps`). */
export type Step = NodeStep | GroupStep;

/** A workflow file (`fx: workflow/v1`). */
export interface WorkflowDocument {
  fx: "workflow/v1";
  /** Lower-case words joined by `-`, at most 96 characters. */
  id: string;
  title: string;
  description?: string | null;
  inputs?: { [name: string]: InputDeclaration };
  /** Constant data, read as `tables.<name>`. */
  tables?: { [name: string]: JsonValue };
  /** Named expressions, read as `let.<name>`. */
  let?: { [name: string]: JsonValue };
  budget?: Budget | null;
  assert?: Assertion[];
  steps: { [name: string]: Step };
  /** What a run returns: expressions over steps and inputs. */
  outputs?: { [name: string]: JsonValue };
  view?: string | null;
}

/** A workflow document: `fields` with `fx: workflow/v1`. */
export function workflow(fields: Omit<WorkflowDocument, "fx">): WorkflowDocument {
  return { fx: "workflow/v1", ...fields };
}

/** `${{ <text> }}`: an expression, to write as a value or inside a string. */
export function expr(text: string): Expression {
  return `\${{ ${text} }}`;
}
