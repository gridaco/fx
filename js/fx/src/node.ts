/**
 * Node types written in TypeScript (`spec/protocol.md` §4), for the TypeScript node host.
 *
 * A declaration is a value; `node()` checks it at once, as the engine checks it while planning,
 * and every static type of the body's `ctx` is inferred from it, with no code generation and no
 * reflection:
 *
 * ```ts
 * import { node } from "@grida/fx";
 *
 * export const badge = node(
 *   {
 *     name: "badge",
 *     description: "Paints one badge.",
 *     inputs: { mask: "image?" },
 *     params: { shape: ["circle", "square"], size: { type: "integer", default: 64, minimum: 16 } },
 *     outputs: { image: "image/png" },
 *   },
 *   async (ctx) => ({ image: ctx.out.png(await paint(ctx.params.shape, ctx.params.size)) }),
 * );
 * ```
 *
 * - An input of one file is an `InputFile`, `kind[]` a readonly array of them, `kind{}` a
 *   `ReadonlyMap` by key (a plain object would reorder keys such as `10` and `2`); a port ending in
 *   `?` is an optional property.
 * - A param is a type name (`"string"`, `"integer"`, `"number"`, `"boolean"`, `"array"`,
 *   `"object"`), a list of choices (their union), or a JSON Schema whose value type is inferred
 *   from `enum`, `const`, `type`, `prefixItems`, `items` and `properties`; an array, an object or
 *   a value no keyword narrows holds JSON values (`JsonValue`), so it can be passed on to a
 *   request, a fact or `ctx.out.json`. `optional: true` and `template: true` are shorthand for
 *   `x-fx-optional` and `x-fx-template`. An optional param is an optional property, with a
 *   `default` or not: a step that binds it to null leaves it out.
 * - The body returns exactly the declared outputs: none missing, none undeclared, each a file
 *   (`ctx.out`, or an `InputFile` passed through). An optional output may be left out or null;
 *   a type with no required outputs may return nothing.
 *
 * Agent tools are declarations too: `tool({ name, description, parameters }, handler)`, whose
 * handler's arguments are typed from `parameters`, a JSON Schema ({@link FromSchema}), for
 * `ctx.agent({ tools })`.
 */

import type { Ctx } from "./ctx.js";
import type { InputFile, Output } from "./files.js";
import { checkValue, isPlainObject, pyRepr, setOwn, typeName } from "./protocol.js";
import { isPattern } from "./schema.js";
import type { JsonInput, JsonValue } from "./workflow.js";

// ------------------------------------------------------------------------------------------------
// Declarations

/** A port family (`spec/protocol.md` §4). */
export type Family = "image" | "audio" | "video" | "model" | "text" | "json" | "annotations" | "file";
/** A file kind: a family, or a media type in it (`image/png`). */
export type Kind = Family | `${Family}/${string}`;
/** A port: one file, a list (`[]`) or a keyed collection (`{}`), optional with a trailing `?`. */
export type Port = Kind | `${Kind}[]` | `${Kind}{}` | `${Kind}?` | `${Kind}[]?` | `${Kind}{}?`;

/** The JSON Schema type names. */
export type JsonSchemaType = "string" | "integer" | "number" | "boolean" | "array" | "object" | "null";

/** A param's JSON Schema (draft 2020-12), with FX's keywords and their shorthand. */
export interface ParamSchema {
  readonly type?: JsonSchemaType | readonly JsonSchemaType[];
  readonly enum?: readonly JsonValue[];
  readonly const?: JsonValue;
  readonly default?: JsonValue;
  readonly prefixItems?: readonly (ParamSchema | boolean)[];
  readonly items?: ParamSchema | boolean;
  readonly properties?: { readonly [name: string]: ParamSchema | boolean };
  readonly required?: readonly string[];
  /** Shorthand for `x-fx-optional`: a step may leave the param out with no default. */
  readonly optional?: boolean;
  /** Shorthand for `x-fx-template`: a string the engine renders as a template or prompt file. */
  readonly template?: boolean;
  readonly "x-fx-optional"?: boolean;
  readonly "x-fx-template"?: boolean;
  readonly [keyword: string]: unknown;
}

/** A param declaration: a type name, a list of choices, or a JSON Schema. */
export type ParamDecl =
  | "string"
  | "integer"
  | "number"
  | "boolean"
  | "array"
  | "object"
  | readonly [string, ...string[]]
  | ParamSchema;

/** Ports by name. */
export type Ports = { readonly [name: string]: Port };
/** Param declarations by name. */
export type ParamDecls = { readonly [name: string]: ParamDecl };

/** A node type's declaration: `spec/protocol.md` §4's fields, with the param shorthand. */
export interface NodeDecl {
  /** Words of `[a-z][a-z0-9_]*` joined by `.`. */
  readonly name: string;
  /** For `grida fx nodes`; never part of an identity. */
  readonly description?: string;
  readonly inputs?: Ports;
  readonly params?: ParamDecls;
  readonly outputs?: Ports;
  /** A judge reports the node fact `verdict`, `accept` or `reject`. */
  readonly judge?: boolean;
  /** The most paid calls of each capability in one run: a number, or an integer param's name. */
  readonly calls?: { readonly [capability: string]: number | string };
  /** Project files the body reads at run time (`ctx.prompt`). */
  readonly resources?: readonly string[];
  /** External programs (`ctx.tool`), optionally with a version bound: `blender>=4.2`. */
  readonly tools?: readonly string[];
  readonly view?: string | null;
  /** Null for a type identified by its export and its source. */
  readonly version?: number | null;
  readonly retry?: "service" | "engine";
}

// ------------------------------------------------------------------------------------------------
// Value types inferred from a declaration

type Bare<P> = P extends `${infer K}?` ? K : P;
type IsOptionalPort<P> = P extends `${string}?` ? true : false;

type InputValue<P> =
  Bare<P> extends `${string}[]`
    ? readonly InputFile[]
    : Bare<P> extends `${string}{}`
      ? ReadonlyMap<string, InputFile>
      : InputFile;

/** What a body may return for one file: a file it made, or one it was handed. */
export type OutputFile = Output | InputFile;

type OutputValue<P> =
  Bare<P> extends `${string}[]`
    ? readonly OutputFile[]
    : Bare<P> extends `${string}{}`
      ? ReadonlyMap<string, OutputFile>
      : OutputFile;

/** The inputs a body receives for these ports. */
export type Inputs<I extends Ports> = {
  readonly [K in keyof I as IsOptionalPort<I[K]> extends true ? never : K]: InputValue<I[K]>;
} & {
  readonly [K in keyof I as IsOptionalPort<I[K]> extends true ? K : never]?: InputValue<I[K]>;
};

/** An output a body may leave out: an optional port, or a one-file port named `annotations`, which
 * the engine writes from the body's marks (`spec/protocol.md` §5.3). */
type MayOmit<K, P> =
  IsOptionalPort<P> extends true
    ? true
    : K extends "annotations"
      ? Bare<P> extends `${string}[]` | `${string}{}`
        ? false
        : true
      : false;

/** The outputs a body returns for these ports. */
export type Outputs<O extends Ports> = {
  [K in keyof O as MayOmit<K, O[K]> extends true ? never : K]: OutputValue<O[K]>;
} & {
  [K in keyof O as MayOmit<K, O[K]> extends true ? K : never]?:
    | OutputValue<O[K]>
    | null
    | undefined;
};

type TypeValue<T, S> =
  T extends "string"
    ? string
    : T extends "integer" | "number"
      ? number
      : T extends "boolean"
        ? boolean
        : T extends "null"
          ? null
          : T extends "array"
            ? ArrayValue<S>
            : T extends "object"
              ? ObjectValue<S>
              : JsonValue;

/** The items `prefixItems` describes, each optional (an array may be shorter). */
type PrefixValues<P> = P extends readonly [infer Head, ...infer Rest]
  ? [SchemaValue<Head>?, ...PrefixValues<Rest>]
  : [];

/** The items after the prefix: as `items` says (none for `items: false`), else any JSON value. */
type RestValues<S> = S extends { readonly items: false }
  ? []
  : S extends { readonly items: infer I }
    ? SchemaValue<I>[]
    : JsonValue[];

type ArrayValue<S> = S extends { readonly prefixItems: infer P extends readonly unknown[] }
  ? readonly [...PrefixValues<P>, ...RestValues<S>]
  : readonly [...RestValues<S>];

type RequiredOf<S> = S extends { readonly required: readonly (infer R)[] } ? R : never;

type ObjectValue<S> = S extends { readonly properties: infer P }
  ? {
      readonly [K in keyof P as K extends RequiredOf<S> ? K : never]: SchemaValue<P[K]>;
    } & {
      readonly [K in keyof P as K extends RequiredOf<S> ? never : K]?: SchemaValue<P[K]>;
    }
  : { readonly [key: string]: JsonValue };

/** The value a JSON Schema describes, as far as its `enum`, `const`, `type`, `prefixItems`,
 * `items` and `properties` say; a JSON value when none of them does. */
export type SchemaValue<S> = S extends { readonly enum: readonly unknown[] }
  ? S["enum"][number]
  : S extends { readonly const: infer C }
    ? C
    : S extends { readonly type: infer T }
      ? TypeValue<T extends readonly unknown[] ? T[number] : T, S>
      : JsonValue;

/** The value of a param with this declaration. */
export type ParamValue<D> = D extends "string"
  ? string
  : D extends "integer" | "number"
    ? number
    : D extends "boolean"
      ? boolean
      : D extends "array"
        ? readonly JsonValue[]
        : D extends "object"
          ? { readonly [key: string]: JsonValue }
          : D extends readonly unknown[]
            ? D[number]
            : SchemaValue<D>;

/** Whether the body may not receive the param: an optional one, default or not, since a step that
 * binds it to null leaves it out (`spec/protocol.md` §5.3). */
type IsOptionalParam<D> = D extends { readonly optional: true } | { readonly "x-fx-optional": true }
  ? true
  : false;

/** The params a body receives for these declarations. */
export type Params<P extends ParamDecls> = {
  readonly [K in keyof P as IsOptionalParam<P[K]> extends true ? never : K]: ParamValue<P[K]>;
} & {
  readonly [K in keyof P as IsOptionalParam<P[K]> extends true ? K : never]?: ParamValue<P[K]>;
};

/** The inputs of a declaration (any input, for a context of no declaration in particular). */
export type InputsOf<D extends NodeDecl> =
  NonNullable<D["inputs"]> extends infer I extends Ports ? Inputs<I> : {};
/** The params of a declaration (any param, for a context of no declaration in particular). */
export type ParamsOf<D extends NodeDecl> =
  NonNullable<D["params"]> extends infer P extends ParamDecls ? Params<P> : {};
/** The outputs of a declaration. */
export type OutputsOf<D extends NodeDecl> = D["outputs"] extends Ports ? Outputs<D["outputs"]> : {};

/** Every member name of `R`, of each type of a union (`keyof` of a union names only the members
 * all its types share, and none once `undefined` or `null` is one of them). */
type AllKeys<R> = R extends unknown ? keyof R : never;

/** No member besides those of `T`: TypeScript does not check a callback's return for excess
 * members, so `node()` constrains it with this. It sees the members of every object a body
 * returns, so an early `return;` or `return null` does not let a misspelled output through. It
 * cannot see a member that TypeScript dropped before: two objects held in variables and returned
 * by one expression (`cond ? a : b`) are reduced to the type of the one with fewer members when
 * it is a subtype of the other; the engine refuses such an output when the body returns it. */
type NoExtra<R, T> = { [K in Exclude<AllKeys<R>, keyof T>]?: never };

/** No declaration field besides those of {@link NodeDecl}: a misspelled field does not compile. */
type NoExtraDecl<D> = { readonly [K in Exclude<keyof D, keyof NodeDecl>]: never };

/** What a body may return: exactly the declared outputs, or nothing when none is required. */
export type BodyReturn<D extends NodeDecl, R> = {} extends OutputsOf<D>
  ? (OutputsOf<D> & NoExtra<R, OutputsOf<D>>) | void | null | undefined
  : OutputsOf<D> & NoExtra<R, OutputsOf<D>>;

// ------------------------------------------------------------------------------------------------
// The type spec

/** A node type as `describe` reports it (`spec/protocol.md` §4). */
export interface TypeSpec {
  readonly name: string;
  readonly description?: string;
  readonly inputs: { readonly [name: string]: string };
  readonly params: { readonly [name: string]: { readonly [keyword: string]: JsonValue } };
  readonly outputs: { readonly [name: string]: string };
  readonly judge: boolean;
  readonly calls: { readonly [capability: string]: number | string };
  readonly resources: readonly string[];
  readonly tools: readonly string[];
  readonly view: string | null;
  readonly version: number | null;
  readonly retry: "service" | "engine";
}

/** The brand of a node type: what the host reads from an export (`NodeRecord`). */
export const NODE: unique symbol = Symbol.for("grida.fx.node") as never;

/** What a node type's brand holds. */
export interface NodeRecord {
  readonly spec: TypeSpec;
  readonly body: (ctx: never) => unknown;
}

/** A declared node type, made by {@link node}. Export it: the engine names it by its module and
 * export (`uses: ./nodes/badges.ts#badge`). */
export interface NodeType<D extends NodeDecl = NodeDecl> {
  /** The type's name. */
  readonly name: D["name"];
  /** The type spec `describe` reports. */
  readonly spec: TypeSpec;
  readonly [NODE]: NodeRecord;
}

/** The node type an export holds, by its brand (any copy of `@grida/fx`); undefined for anything
 * else. */
export function nodeRecordOf(value: unknown): NodeRecord | undefined {
  if ((typeof value !== "object" && typeof value !== "function") || value === null) {
    return undefined;
  }
  let record: unknown;
  try {
    record = (value as Record<symbol, unknown>)[NODE];
  } catch {
    return undefined;
  }
  if (typeof record !== "object" || record === null) {
    return undefined;
  }
  const { spec, body } = record as { spec?: unknown; body?: unknown };
  if (typeof body !== "function" || typeof spec !== "object" || spec === null) {
    return undefined;
  }
  return record as NodeRecord;
}

/**
 * Declares a node type. The declaration is checked at once (an invalid one throws, so the module
 * fails to load and `describe` reports why); the body gets a `ctx` typed from it and returns
 * exactly the declared outputs. (A body that returns a promise and one that does not are two
 * signatures, so the outputs a promise resolves to are inferred as they are, and held to the
 * declaration the same way.)
 */
export function node<const D extends NodeDecl, R extends BodyReturn<D, R>>(
  decl: D & NoExtraDecl<D>,
  body: (ctx: Ctx<D>) => Promise<R>,
): NodeType<D>;
/** Declares a node type whose body returns its outputs at once, not a promise of them (the
 * signature above has the details). */
export function node<const D extends NodeDecl, R extends BodyReturn<D, R>>(
  decl: D & NoExtraDecl<D>,
  body: (ctx: Ctx<D>) => R,
): NodeType<D>;
export function node(decl: NodeDecl, body: (ctx: never) => unknown): NodeType {
  if (typeof body !== "function") {
    throw new TypeError(`a node body is a function, not ${typeName(body)}`);
  }
  const spec = declare(decl);
  const record: NodeRecord = Object.freeze({ spec, body: body as (ctx: never) => unknown });
  const made = { name: spec.name, spec };
  Object.defineProperty(made, NODE, { value: record });
  return Object.freeze(made) as unknown as NodeType;
}

// ------------------------------------------------------------------------------------------------
// Agent tools

/** A JSON Schema (draft 2020-12) as a value: a tool's `parameters`, an agent's `submit`. Written
 * directly in a call (or `as const`), its literal types give the value it describes its type
 * ({@link FromSchema}). */
export interface JsonSchemaObject {
  readonly type?: JsonSchemaType | readonly JsonSchemaType[];
  readonly enum?: readonly JsonValue[];
  readonly const?: JsonValue;
  readonly properties?: { readonly [name: string]: JsonSchemaObject | boolean };
  readonly required?: readonly string[];
  readonly additionalProperties?: JsonSchemaObject | boolean;
  readonly items?: JsonSchemaObject | boolean;
  readonly anyOf?: readonly (JsonSchemaObject | boolean)[];
  readonly oneOf?: readonly (JsonSchemaObject | boolean)[];
  readonly description?: string;
  readonly [keyword: string]: unknown;
}

type Simplify<T> = { [K in keyof T]: T[K] } & {};

type FromType<T, S> = T extends "string"
  ? string
  : T extends "integer" | "number"
    ? number
    : T extends "boolean"
      ? boolean
      : T extends "null"
        ? null
        : T extends "array"
          ? S extends { readonly items: infer I }
            ? readonly FromSchema<I>[]
            : readonly JsonValue[]
          : T extends "object"
            ? FromObject<S>
            : unknown;

type FromObject<S> = S extends { readonly properties: infer P }
  ? Simplify<
      {
        readonly [K in keyof P as K extends RequiredOf<S> ? K : never]: FromSchema<P[K]>;
      } & {
        readonly [K in keyof P as K extends RequiredOf<S> ? never : K]?: FromSchema<P[K]>;
      }
    >
  : S extends { readonly additionalProperties: infer A }
    ? A extends false
      ? {}
      : { readonly [key: string]: FromSchema<A> }
    : { readonly [key: string]: JsonValue };

/**
 * The value a JSON Schema describes, from its `const`, `enum`, `anyOf`, `oneOf`, `type` (one or
 * several), `items`, `properties`, `required` and `additionalProperties`; `unknown` where it says
 * nothing. A property not `required` is optional. Nothing checks a value against the schema at
 * run time: the engine validates an agent's submitted value, and a tool's arguments are the
 * model's.
 */
export type FromSchema<S> = S extends true
  ? unknown
  : S extends false
    ? never
    : S extends { readonly const: infer C }
      ? C
      : S extends { readonly enum: readonly (infer E)[] }
        ? E
        : S extends { readonly anyOf: readonly (infer A)[] }
          ? FromSchema<A>
          : S extends { readonly oneOf: readonly (infer A)[] }
            ? FromSchema<A>
            : S extends { readonly type: infer T }
              ? FromType<T extends readonly unknown[] ? T[number] : T, S>
              : S extends { readonly properties: unknown }
                ? FromObject<S>
                : unknown;

/** A tool's arguments, as its parameters describe them: always an object of JSON values (the
 * engine passes the model's arguments as an object, whatever `parameters` says). */
export type ToolArguments<P> = unknown extends FromSchema<P>
  ? { readonly [name: string]: JsonValue }
  : FromSchema<P> extends readonly unknown[]
    ? { readonly [name: string]: JsonValue }
    : FromSchema<P> extends object
      ? FromSchema<P>
      : { readonly [name: string]: JsonValue };

/** A picture shown to an agent: a file the run was handed, a file the body made, a path (or a
 * `file:` URL), or a `data:` URL. */
export type Picture = InputFile | Output | string | URL;

/** The brand of an agent tool: what the host reads from it (`ToolRecord`). */
export const AGENT_TOOL: unique symbol = Symbol.for("grida.fx.tool") as never;
const TOOL_REPLY = Symbol.for("grida.fx.tool_reply");

/** What a tool returns: a JSON value, which is the model's `content`, or a {@link ToolReply} with
 * pictures. Nothing is `null`; any other value is sent as its string. */
export type ToolReturn = JsonInput | ToolReply | void;

/** A tool's answer with pictures: `{content: text, images}` (`spec/protocol.md` §5.4). */
export class ToolReply {
  readonly text: string;
  readonly images: readonly Picture[];

  constructor(text: string, images: readonly Picture[] = []) {
    if (!Array.isArray(images)) {
      throw new TypeError(`a tool reply's images are a list of pictures, not ${typeName(images)}`);
    }
    this.text = text;
    this.images = Object.freeze([...images]);
    Object.defineProperty(this, TOOL_REPLY, { value: true });
  }
}

/** A {@link ToolReply} of any copy of `@grida/fx`; undefined for anything else. */
export function toolReplyOf(
  value: unknown,
): { readonly text: unknown; readonly images: readonly unknown[] } | undefined {
  if (typeof value !== "object" || value === null) {
    return undefined;
  }
  try {
    const reply = value as { [TOOL_REPLY]?: unknown; text?: unknown; images?: unknown };
    if (reply[TOOL_REPLY] !== true) {
      return undefined;
    }
    const images = reply.images;
    return { text: reply.text, images: Array.isArray(images) ? images : [] };
  } catch {
    return undefined;
  }
}

/** What a tool's brand holds. */
export interface ToolRecord {
  readonly name: string;
  readonly description: string;
  readonly parameters: JsonSchemaObject;
  readonly handler: (args: never, ctx: never) => unknown;
}

/** A declared agent tool, made by {@link tool}: give it to `ctx.agent({ tools })`. */
export interface Tool<P extends JsonSchemaObject = JsonSchemaObject> {
  readonly name: string;
  readonly description: string;
  readonly parameters: P;
  readonly [AGENT_TOOL]: ToolRecord;
}

/** A tool's declaration: its name (`lower_snake_case`, at most 64 characters, never `submit`), what
 * it does, for the model, and its parameters as a JSON Schema. */
export interface ToolDecl<P extends JsonSchemaObject> {
  readonly name: string;
  readonly description: string;
  readonly parameters: P;
}

/** The tool an agent calls to finish; the engine adds it when `submit` is given. */
export const SUBMIT = "submit";
const TOOL_NAME = /^[a-z][a-z0-9_]{0,63}$/;
const TOOL_FIELDS = new Set(["name", "description", "parameters"]);

/**
 * Declares an agent tool. The host calls `handler(arguments, ctx)` when the model calls it
 * (`tool.invoke`), with the arguments the model filled in, typed from `parameters`, and the body's
 * own `ctx` (`ctx.state` is shared). What it returns is the model's answer ({@link ToolReturn}); a
 * `NodeFailure` it throws fails the node, and anything else it throws is told to the model as
 * `<name>: <message>`, and the loop goes on.
 */
export function tool<const P extends JsonSchemaObject>(
  decl: ToolDecl<P>,
  handler: (args: ToolArguments<P>, ctx: Ctx) => ToolReturn | Promise<ToolReturn>,
): Tool<P> {
  if (!isPlainObject(decl)) {
    throw new TypeError(`a tool declaration is an object, not ${typeName(decl)}`);
  }
  for (const field of Object.keys(decl)) {
    if (!TOOL_FIELDS.has(field)) {
      throw new TypeError(`a tool declaration has no field ${pyRepr(field)}`);
    }
  }
  const { name, description, parameters } = decl as Record<string, unknown>;
  if (typeof name !== "string" || !TOOL_NAME.test(name)) {
    throw new TypeError("tool name must be lower_snake_case, at most 64 characters");
  }
  if (typeof description !== "string" || !description.trim()) {
    throw new TypeError(`tool ${name} must carry a description`);
  }
  if (!isPlainObject(parameters)) {
    throw new TypeError(`tool ${name} parameters must be a JSON Schema object`);
  }
  try {
    checkValue(parameters);
  } catch (error) {
    const why = (error as Error).message;
    throw new TypeError(`tool ${name} parameters must be a JSON Schema object: ${why}`);
  }
  if (name === SUBMIT) {
    throw new TypeError(`${SUBMIT} is reserved for the episode's final answer`);
  }
  if (typeof handler !== "function") {
    throw new TypeError(`tool ${name} handler must be a function`);
  }
  const kept = freeze(structuredClone(parameters)) as P;
  const record: ToolRecord = Object.freeze({
    name,
    description,
    parameters: kept,
    handler: handler as (args: never, ctx: never) => unknown,
  });
  const made = { name, description, parameters: kept };
  Object.defineProperty(made, AGENT_TOOL, { value: record });
  return Object.freeze(made) as unknown as Tool<P>;
}

/** The tool a value is, by its brand (any copy of `@grida/fx`); undefined for anything else. */
export function toolRecordOf(value: unknown): ToolRecord | undefined {
  if ((typeof value !== "object" && typeof value !== "function") || value === null) {
    return undefined;
  }
  let record: unknown;
  try {
    record = (value as Record<symbol, unknown>)[AGENT_TOOL];
  } catch {
    return undefined;
  }
  if (typeof record !== "object" || record === null) {
    return undefined;
  }
  const { name, description, parameters, handler } = record as Record<string, unknown>;
  if (
    typeof name !== "string" ||
    typeof description !== "string" ||
    !isPlainObject(parameters) ||
    typeof handler !== "function"
  ) {
    return undefined;
  }
  return record as ToolRecord;
}

// ------------------------------------------------------------------------------------------------
// Checking a declaration

/** A node type declaration that cannot be planned against. */
export class SpecError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SpecError";
  }
}

const FAMILIES = ["annotations", "audio", "file", "image", "json", "model", "text", "video"];
const PORT = /^(?<kind>[a-z0-9]+(?:\/[a-z0-9.+-]+)?)(?<shape>\[\]|\{\})?(?<optional>\?)?$/;
const TYPE_NAME = /^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*$/;
const NAME = /^[a-z][a-z0-9_]*$/;
const SEGMENT = String.raw`(?:[^/\\.][^/\\]*|\.[^/\\.][^/\\]*|\.\.[^/\\]+)`;
/** A project path: POSIX, relative, with no empty, `.` or `..` segment and no backslash. */
export const PROJECT_PATH = new RegExp(`^${SEGMENT}(?:/${SEGMENT})*$`);
const TOOL = /^[a-z0-9][a-z0-9_-]*(?:\s*[<>=!~].*)?$/;
const TYPE_NAMES: Readonly<Record<string, string>> = {
  string: "string",
  integer: "integer",
  number: "number",
  boolean: "boolean",
  array: "array",
  object: "object",
};
const RENAMED_KEYS: Readonly<Record<string, string>> = {
  optional: "x-fx-optional",
  template: "x-fx-template",
};
/** A type spec's fields after `name` and `description`, in the protocol's order (`spec/protocol.md`
 * §4). */
export const SPEC_FIELDS: readonly string[] = [
  "inputs",
  "params",
  "outputs",
  "judge",
  "calls",
  "resources",
  "tools",
  "view",
  "version",
  "retry",
];
const FIELDS = new Set(["name", "description", ...SPEC_FIELDS]);

/** A port's parts: its kind, its shape and whether it is optional. */
export interface PortShape {
  readonly kind: string;
  readonly shape: "one" | "list" | "keyed";
  readonly optional: boolean;
}

/** Parses `kind`, `kind[]`, `kind{}` with an optional `?`. */
export function parsePort(notation: unknown): PortShape {
  const match = typeof notation === "string" ? PORT.exec(notation) : null;
  if (match === null) {
    throw new SpecError(`port ${pyRepr(notation)} is not kind, kind[], kind{} with an optional ?`);
  }
  const groups = match.groups as { kind: string; shape?: string; optional?: string };
  const kind = groups.kind;
  if (!FAMILIES.includes(kind.split("/", 1)[0] as string)) {
    throw new SpecError(`port kind ${pyRepr(kind)} is not one of ${pyRepr(FAMILIES)}`);
  }
  const shape = groups.shape === "[]" ? "list" : groups.shape === "{}" ? "keyed" : "one";
  return { kind, shape, optional: groups.optional !== undefined };
}

/** A param declaration as JSON Schema (module comment). */
export function paramSchema(declared: unknown): Record<string, unknown> {
  if (typeof declared === "string" && Object.hasOwn(TYPE_NAMES, declared)) {
    return { type: TYPE_NAMES[declared] };
  }
  if (Array.isArray(declared)) {
    return { enum: [...declared] };
  }
  if (isPlainObject(declared)) {
    const built: Record<string, unknown> = {};
    for (const [key, given] of Object.entries(declared)) {
      if (given === undefined) {
        continue; // left out, as JSON leaves it out
      }
      const value = withoutUndefined(given);
      const renamed = typeof value === "boolean" ? RENAMED_KEYS[key] : undefined;
      if (renamed !== undefined) {
        if (Object.hasOwn(declared, renamed) && declared[renamed] !== value) {
          throw new SpecError(`param schema sets both ${key} and ${renamed}`);
        }
        built[renamed] = value;
      } else if (Object.hasOwn(built, key) && Object.values(RENAMED_KEYS).includes(key)) {
        // already written from the shorthand, with the same value
      } else {
        built[key] = value;
      }
    }
    return built;
  }
  throw new SpecError(
    `param declaration ${pyRepr(declared)} is a type name, a list of choices or a schema`,
  );
}

/** A value with every object member whose value is undefined left out, at any depth, as JSON
 * leaves it out (an undefined array item stays, and is refused). */
function withoutUndefined(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => withoutUndefined(item));
  }
  if (isPlainObject(value)) {
    const kept: Record<string, unknown> = {};
    for (const [key, member] of Object.entries(value)) {
      if (member !== undefined) {
        setOwn(kept, key, withoutUndefined(member));
      }
    }
    return kept;
  }
  return value;
}

/** The type spec of a declaration, checked (`SpecError` with the engine's sentences). */
export function declare(decl: unknown): TypeSpec {
  if (!isPlainObject(decl)) {
    throw new TypeError(`a node declaration is an object, not ${typeName(decl)}`);
  }
  const name = decl.name;
  for (const field of Object.keys(decl)) {
    if (!FIELDS.has(field)) {
      throw new SpecError(`${String(name)}: a node declaration has no field ${pyRepr(field)}`);
    }
  }
  const absent = (field: string) => decl[field] === undefined;
  if (!absent("params") && !isPlainObject(decl.params)) {
    throw new SpecError(`${String(name)}: params map names to declarations`);
  }
  if (!absent("calls") && !isPlainObject(decl.calls)) {
    throw new SpecError(`${String(name)}: calls map capabilities to bounds`);
  }
  const inputs = ports(name, "inputs", decl.inputs);
  const params: Record<string, unknown> = {};
  for (const [key, value] of Object.entries((decl.params ?? {}) as Record<string, unknown>)) {
    if (value !== undefined) {
      params[key] = paramSchema(value);
    }
  }
  const outputs = ports(name, "outputs", decl.outputs);
  const spec: Record<string, unknown> = { name };
  const description = decl.description;
  if (description !== undefined && description !== null) {
    if (typeof description !== "string") {
      throw new SpecError(`${String(name)}: description must be a string or null`);
    }
    if (description.trim()) {
      spec.description = description;
    }
  }
  spec.inputs = inputs;
  spec.params = params;
  spec.outputs = outputs;
  spec.judge = decl.judge ?? false;
  const calls = Object.entries((decl.calls ?? {}) as Record<string, unknown>);
  spec.calls = Object.fromEntries(calls.filter(([, bound]) => bound !== undefined));
  spec.resources = sequence(name, "resources", decl.resources);
  spec.tools = sequence(name, "tools", decl.tools);
  spec.view = decl.view ?? null;
  spec.version = decl.version ?? null;
  spec.retry = decl.retry ?? "service";
  validateTypeSpec(spec);
  return freeze(spec) as unknown as TypeSpec;
}

function ports(name: unknown, role: string, declared: unknown): Record<string, string> {
  if (declared === undefined || declared === null) {
    return {};
  }
  if (!isPlainObject(declared)) {
    throw new SpecError(`${String(name)}: ${role} map names to port notation`);
  }
  const built: Record<string, string> = {};
  for (const [key, value] of Object.entries(declared)) {
    if (value !== undefined) {
      parsePort(value);
      built[key] = value as string;
    }
  }
  return built;
}

function sequence(name: unknown, role: string, values: unknown): unknown {
  if (values === undefined || values === null) {
    return [];
  }
  if (typeof values === "string") {
    throw new SpecError(`${String(name)}: ${role} is a list of strings, not one string`);
  }
  return Array.isArray(values) ? [...values] : values;
}

function freeze<T>(value: T): T {
  if (typeof value === "object" && value !== null && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const item of Object.values(value)) {
      freeze(item);
    }
  }
  return value;
}

/**
 * Throws a {@link SpecError} for anything `spec/protocol.md` §4 refuses in a type spec, with the
 * engine's sentences: the name, the names and ports of inputs, params and outputs, params as JSON
 * Schema objects, the calls' bounds, resources, tools, `judge`, `view`, `version`, `retry` and
 * `description`. `describe` checks every type it reports again, so a spec made by another copy of
 * `@grida/fx` is held to the same rules.
 */
export function validateTypeSpec(spec: Record<string, unknown>): void {
  const name = spec.name;
  if (typeof name !== "string" || !TYPE_NAME.test(name)) {
    throw new SpecError(`node type name ${pyRepr(name)} must be lower_snake words joined by .`);
  }
  const entries: [string, unknown][] = [
    ["input", spec.inputs],
    ["param", spec.params],
    ["output", spec.outputs],
  ];
  for (const [role, declared] of entries) {
    if (!isPlainObject(declared)) {
      throw new SpecError(`${name}: ${role}s map names to declarations`);
    }
    for (const key of Object.keys(declared)) {
      if (!NAME.test(key)) {
        throw new SpecError(
          `${name}: ${role} name ${pyRepr(key)} must be one lower_snake word ` +
            "(a letter, then letters, digits or _)",
        );
      }
    }
  }
  const inputs = spec.inputs as Record<string, unknown>;
  const params = spec.params as Record<string, unknown>;
  const outputs = spec.outputs as Record<string, unknown>;
  for (const [role, declared] of [
    ["input", inputs],
    ["output", outputs],
  ] as const) {
    for (const [key, port] of Object.entries(declared)) {
      try {
        parsePort(port);
      } catch {
        throw new SpecError(`${name}: ${role} ${pyRepr(key)} is not a port`);
      }
    }
  }
  const overlap = Object.keys(inputs)
    .filter((key) => Object.hasOwn(params, key))
    .sort();
  if (overlap.length > 0) {
    throw new SpecError(`${name}: ${pyRepr(overlap)} declared as both input and param`);
  }
  for (const [key, schema] of Object.entries(params)) {
    checkParam(name, key, schema);
  }
  checkCalls(name, spec.calls, params);
  checkPaths(
    name,
    "resource",
    spec.resources,
    PROJECT_PATH,
    "a POSIX path relative to the project root, without ./ or .. segments",
  );
  checkPaths(
    name,
    "tool",
    spec.tools,
    TOOL,
    "a program name, optionally followed by a version bound such as blender>=4.2",
  );
  if (typeof spec.judge !== "boolean") {
    throw new SpecError(`${name}: judge must be true or false`);
  }
  if (spec.view !== null && typeof spec.view !== "string") {
    throw new SpecError(`${name}: view must be a string or null`);
  }
  const version = spec.version;
  if (version !== null && (typeof version !== "number" || !Number.isInteger(version) || version < 0)) {
    throw new SpecError(`${name}: version must be a whole number of at least 0, or null`);
  }
  if (spec.retry !== "service" && spec.retry !== "engine") {
    throw new SpecError(`${name}: retry is 'service' or 'engine', not ${pyRepr(spec.retry)}`);
  }
  if (spec.description !== undefined && typeof spec.description !== "string") {
    throw new SpecError(`${name}: description must be a string or null`);
  }
  try {
    checkValue(spec);
  } catch (error) {
    throw new SpecError(`${name}: ${(error as Error).message}`);
  }
}

function checkParam(name: string, key: string, schema: unknown): void {
  if (!isPlainObject(schema)) {
    throw new SpecError(`${name}: param ${pyRepr(key)} must be a JSON Schema object`);
  }
  for (const keyword of ["x-fx-optional", "x-fx-template"]) {
    if (Object.hasOwn(schema, keyword) && typeof schema[keyword] !== "boolean") {
      throw new SpecError(`${name}: param ${pyRepr(key)}: ${keyword} must be true or false`);
    }
  }
  try {
    checkValue(schema);
  } catch (error) {
    const why = (error as Error).message;
    throw new SpecError(`${name}: param ${pyRepr(key)} is not a JSON value: ${why}`);
  }
  for (const pattern of patternsOf(schema)) {
    if (typeof pattern === "string" && !isPattern(pattern)) {
      throw new SpecError(
        `${name}: param ${pyRepr(key)}: the pattern ${pyRepr(pattern)} is not a regular expression`,
      );
    }
  }
}

/** The patterns the host's params check reads in a schema (`schema.ts`): each `pattern` and each
 * `patternProperties` name, in its subschemas too. */
function* patternsOf(schema: unknown): Generator<unknown> {
  if (!isPlainObject(schema)) {
    return;
  }
  if (Object.hasOwn(schema, "pattern")) {
    yield schema.pattern;
  }
  const { properties, patternProperties, additionalProperties, items, prefixItems } = schema;
  if (isPlainObject(patternProperties)) {
    yield* Object.keys(patternProperties);
  }
  const nested = [
    ...(isPlainObject(properties) ? Object.values(properties) : []),
    ...(isPlainObject(patternProperties) ? Object.values(patternProperties) : []),
    ...(Array.isArray(prefixItems) ? prefixItems : []),
    additionalProperties,
    items,
  ];
  for (const subschema of nested) {
    yield* patternsOf(subschema);
  }
}

function checkCalls(name: string, calls: unknown, params: Record<string, unknown>): void {
  if (!isPlainObject(calls)) {
    throw new SpecError(`${name}: calls map capabilities to bounds`);
  }
  for (const [capability, count] of Object.entries(calls)) {
    if (!capability) {
      throw new SpecError(`${name}: calls are keyed by capability names`);
    }
    const where = `${name}: calls[${pyRepr(capability)}]`;
    if (typeof count === "string") {
      const setting = (isPlainObject(params[count]) ? params[count] : {}) as Record<string, unknown>;
      const minimum = Object.hasOwn(setting, "minimum") ? setting.minimum : 0;
      if (
        !NAME.test(count) ||
        setting.type !== "integer" ||
        typeof minimum !== "number" ||
        minimum < 1
      ) {
        throw new SpecError(
          `${where} names ${pyRepr(count)}, which is not an integer setting with a minimum of ` +
            "at least 1",
        );
      }
      if (typeof setting.maximum !== "number") {
        throw new SpecError(`${where} needs ${pyRepr(count)} to set a maximum`);
      }
    } else if (typeof count !== "number" || !Number.isInteger(count)) {
      throw new SpecError(`${where} is a whole number of at least 1 or the name of an integer param`);
    } else if (count < 1) {
      throw new SpecError(`${where} must be at least 1`);
    } else {
      try {
        checkValue(count);
      } catch (error) {
        throw new SpecError(`${where}: ${(error as Error).message}`);
      }
    }
  }
}

function checkPaths(name: string, role: string, values: unknown, pattern: RegExp, what: string): void {
  if (!Array.isArray(values)) {
    throw new SpecError(`${name}: ${role}s are a list of strings`);
  }
  const seen = new Set<string>();
  for (const value of values) {
    if (typeof value !== "string" || !pattern.test(value)) {
      throw new SpecError(`${name}: ${role} ${pyRepr(value)} must be ${what}`);
    }
    if (seen.has(value)) {
      throw new SpecError(`${name}: ${role} ${pyRepr(value)} is declared twice`);
    }
    seen.add(value);
  }
}
