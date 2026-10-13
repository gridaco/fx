/**
 * Param values checked against their declared schemas (`spec/protocol.md` §5.3), so the static
 * types `node()` infers for `ctx.params` hold when the body runs. The engine fills defaults in and
 * renders templates, but never checks a value against its schema: a step's `with:` may bind an
 * expression to a param, and a missing value arrives as `null`.
 *
 * - A `null` for an optional param (`x-fx-optional`) means the param is absent: it is left out.
 * - Any other value is checked against the param's JSON Schema (draft 2020-12) with these
 *   keywords: `type` (`integer` too, a list of types, `null`), `enum`, `const`, `minimum`,
 *   `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`, `minLength`, `maxLength`,
 *   `pattern`, `items`, `prefixItems`, `minItems`, `maxItems`, `uniqueItems`, `properties`,
 *   `patternProperties`, `required`, `additionalProperties` (a boolean or a schema),
 *   `minProperties` and `maxProperties`. Other keywords are ignored. A value at a `param_files`
 *   pointer (a file) is not checked itself; the keywords of the places that hold it (`enum`,
 *   `const`, `uniqueItems`) see it as its plain projection, `{"file": "<digest>"}`.
 * - `multipleOf` is exact in decimal, as the engine's check is (0.7 is a multiple of 0.1).
 * - A `pattern` (or a `patternProperties` name) is an ECMA-262 regular expression, read with the
 *   `u` flag, else without it, as the engine reads hand-written patterns such as `^\d{3}\-\d{4}$`;
 *   `node()` refuses one that is neither.
 * - A value that fails fails the node: `param <name>: <message>`, or `param <name>/<path>:
 *   <message>` below its top, for the first of its errors in §6.2's order (by place, then by
 *   keyword), worded as §6.2's table words them, values written as canonical JSON
 *   (`"triangle" is not one of ["circle","square"]`, `"big" is not of type "integer"`).
 */

import { NodeFailure } from "./engine-errors.js";
import { isPlainObject, placeAt, pointerToken, setOwn } from "./protocol.js";

/** A segment of a place in a value (an array index or a member name) or in a schema. */
type Segment = string | number;

/** One keyword a value fails. */
export interface SchemaError {
  /** Where in the value: its segments from the top. */
  readonly where: readonly Segment[];
  /** Where in the schema: the keyword's evaluation path. */
  readonly keyword: readonly Segment[];
  readonly message: string;
}

/**
 * The params of a run as the body receives them: each checked against its declared schema (module
 * comment), and a `null` for an optional param left out. `files` are the `param_files` pointers,
 * each with its file's digest. A {@link NodeFailure} names the first param that fails, in the
 * order the params are declared.
 */
export function checkedParams(
  params: Readonly<Record<string, unknown>>,
  declared: Readonly<Record<string, unknown>>,
  files: Readonly<Record<string, string>> = {},
): Record<string, unknown> {
  const checked: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(params)) {
    setOwn(checked, name, value);
  }
  const skipped = new Set(Object.keys(files));
  const seen = projectFiles(params, files);
  for (const [name, schema] of Object.entries(declared)) {
    if (!Object.hasOwn(params, name)) {
      continue;
    }
    const value = seen[name];
    if (value === null && isPlainObject(schema) && schema["x-fx-optional"] === true) {
      delete checked[name];
      continue;
    }
    const [first] = schemaErrors(value, schema as Schema, [name], skipped);
    if (first !== undefined) {
      const path = first.where.map(String).join("/");
      throw new NodeFailure(`param ${path}: ${first.message}`);
    }
  }
  return checked;
}

/** The params with each file's plain projection, `{"file": <digest>}`, at its pointer (one that
 * names no place is passed over: the context refuses it when the body reads its params). */
function projectFiles(
  params: Readonly<Record<string, unknown>>,
  files: Readonly<Record<string, string>>,
): Record<string, unknown> {
  if (Object.keys(files).length === 0) {
    return { ...params };
  }
  const copy = structuredClone({ ...params }) as Record<string, unknown>;
  for (const [pointer, digest] of Object.entries(files)) {
    placeAt(copy, pointer, { file: digest });
  }
  return copy;
}

/** A JSON Schema: an object of keywords, or `true` / `false`. */
export type Schema = boolean | { readonly [keyword: string]: unknown };

/**
 * Every keyword `value` fails in `schema` (module comment), in §6.2's order: by place in the value
 * (array indexes by number, member names by their text, a place before the longer places it
 * starts), then by keyword path. `where` is the place the value sits at; a place whose JSON
 * pointer (its segments from the top of `where`) is in `skip` is not checked.
 */
export function schemaErrors(
  value: unknown,
  schema: Schema,
  where: readonly Segment[] = [],
  skip: ReadonlySet<string> = new Set(),
): SchemaError[] {
  const errors: SchemaError[] = [];
  check(value, schema, [...where], [], errors, skip);
  return errors.sort(
    (a, b) => compareSegments(a.where, b.where) || compareSegments(a.keyword, b.keyword),
  );
}

function check(
  value: unknown,
  schema: unknown,
  where: Segment[],
  at: Segment[],
  errors: SchemaError[],
  skip: ReadonlySet<string>,
): void {
  if (skip.size > 0 && skip.has(pointer(where))) {
    return;
  }
  if (schema === true) {
    return;
  }
  const fail = (keyword: Segment[], message: string, place: Segment[] = where) =>
    errors.push({ where: place, keyword: [...at, ...keyword], message });
  if (schema === false) {
    fail([], `${canonicalJson(value)} is not allowed`);
    return;
  }
  if (!isPlainObject(schema)) {
    return;
  }
  const v = () => canonicalJson(value);
  const types = typeof schema.type === "string" ? [schema.type] : Array.isArray(schema.type) ? schema.type : null;
  if (types !== null && !types.some((type) => hasType(value, type))) {
    const names = types
      .filter((type): type is string => typeof type === "string")
      .sort()
      .map((type) => canonicalJson(type));
    fail(["type"], `${v()} is not of type ${names.join(" or ")}`);
  }
  if (Array.isArray(schema.enum) && !schema.enum.some((option) => equal(option, value))) {
    fail(["enum"], `${v()} is not one of ${canonicalJson(schema.enum)}`);
  }
  // A keyword set to undefined is absent, as JSON leaves it out.
  if (schema.const !== undefined && !equal(schema.const, value)) {
    fail(["const"], `${v()} is not ${canonicalJson(schema.const)}`);
  }
  if (typeof value === "number") {
    checkNumber(value, schema, fail, v);
  } else if (typeof value === "string") {
    checkString(value, schema, fail, v);
  } else if (Array.isArray(value)) {
    checkArray(value, schema, where, at, errors, skip, fail, v);
  } else if (isPlainObject(value)) {
    checkObject(value, schema, where, at, errors, skip, fail, v);
  }
}

type Fail = (keyword: Segment[], message: string, place?: Segment[]) => void;

function checkNumber(
  value: number,
  schema: Record<string, unknown>,
  fail: Fail,
  v: () => string,
): void {
  const { minimum, maximum, exclusiveMinimum, exclusiveMaximum, multipleOf } = schema;
  if (typeof minimum === "number" && value < minimum) {
    fail(["minimum"], `${v()} is less than the minimum ${canonicalJson(minimum)}`);
  }
  if (typeof maximum === "number" && value > maximum) {
    fail(["maximum"], `${v()} is greater than the maximum ${canonicalJson(maximum)}`);
  }
  if (typeof exclusiveMinimum === "number" && value <= exclusiveMinimum) {
    fail(["exclusiveMinimum"], `${v()} is not greater than ${canonicalJson(exclusiveMinimum)}`);
  }
  if (typeof exclusiveMaximum === "number" && value >= exclusiveMaximum) {
    fail(["exclusiveMaximum"], `${v()} is not less than ${canonicalJson(exclusiveMaximum)}`);
  }
  if (typeof multipleOf === "number" && multipleOf > 0 && !isMultiple(value, multipleOf)) {
    fail(["multipleOf"], `${v()} is not a multiple of ${canonicalJson(multipleOf)}`);
  }
}

function checkString(
  value: string,
  schema: Record<string, unknown>,
  fail: Fail,
  v: () => string,
): void {
  const { minLength, maxLength, pattern } = schema;
  // JSON Schema counts characters as code points.
  const length = [...value].length;
  if (typeof minLength === "number" && length < minLength) {
    fail(["minLength"], `${v()} is shorter than ${count(minLength, "character", "characters")}`);
  }
  if (typeof maxLength === "number" && length > maxLength) {
    fail(["maxLength"], `${v()} is longer than ${count(maxLength, "character", "characters")}`);
  }
  if (typeof pattern === "string") {
    const expression = regex(pattern);
    if (expression === null) {
      fail(["pattern"], `the pattern ${canonicalJson(pattern)} is not a regular expression`);
    } else if (!expression.test(value)) {
      fail(["pattern"], `${v()} does not match the pattern ${canonicalJson(pattern)}`);
    }
  }
}

function checkArray(
  value: readonly unknown[],
  schema: Record<string, unknown>,
  where: Segment[],
  at: Segment[],
  errors: SchemaError[],
  skip: ReadonlySet<string>,
  fail: Fail,
  v: () => string,
): void {
  const { prefixItems, items, minItems, maxItems, uniqueItems } = schema;
  let start = 0;
  if (Array.isArray(prefixItems)) {
    start = prefixItems.length;
    prefixItems.forEach((itemSchema, index) => {
      if (index < value.length) {
        check(value[index], itemSchema, [...where, index], [...at, "prefixItems", index], errors, skip);
      }
    });
  }
  // `items: false` is a false schema for each item after the prefix: `<v> is not allowed` at each.
  if (items !== undefined) {
    for (let index = start; index < value.length; index += 1) {
      check(value[index], items, [...where, index], [...at, "items"], errors, skip);
    }
  }
  if (typeof minItems === "number" && value.length < minItems) {
    fail(["minItems"], `${v()} has fewer than ${count(minItems, "item", "items")}`);
  }
  if (typeof maxItems === "number" && value.length > maxItems) {
    fail(["maxItems"], `${v()} has more than ${count(maxItems, "item", "items")}`);
  }
  if (uniqueItems === true) {
    const seen = new Set(value.map((item) => canonicalJson(item)));
    if (seen.size < value.length) {
      fail(["uniqueItems"], `${v()} has repeated items`);
    }
  }
}

function checkObject(
  value: Record<string, unknown>,
  schema: Record<string, unknown>,
  where: Segment[],
  at: Segment[],
  errors: SchemaError[],
  skip: ReadonlySet<string>,
  fail: Fail,
  v: () => string,
): void {
  const { properties, patternProperties, additionalProperties, required, minProperties, maxProperties } =
    schema;
  const names = Object.keys(value).filter((name) => value[name] !== undefined);
  const covered = new Set<string>();
  if (isPlainObject(properties)) {
    for (const [name, propertySchema] of Object.entries(properties)) {
      if (Object.hasOwn(value, name) && value[name] !== undefined) {
        covered.add(name);
        check(value[name], propertySchema, [...where, name], [...at, "properties", name], errors, skip);
      }
    }
  }
  if (isPlainObject(patternProperties)) {
    for (const [pattern, patternSchema] of Object.entries(patternProperties)) {
      const expression = regex(pattern);
      if (expression === null) {
        const why = `the pattern ${canonicalJson(pattern)} is not a regular expression`;
        fail(["patternProperties", pattern], why);
        continue;
      }
      for (const name of names) {
        if (expression.test(name)) {
          covered.add(name);
          check(value[name], patternSchema, [...where, name], [...at, "patternProperties", pattern], errors, skip);
        }
      }
    }
  }
  const additional = names.filter((name) => !covered.has(name));
  if (additionalProperties === false && additional.length > 0) {
    const listed = [...additional].sort().map((name) => canonicalJson(name));
    fail(
      ["additionalProperties"],
      listed.length === 1
        ? `the property ${listed[0]} is not allowed`
        : `the properties ${listed.join(", ")} are not allowed`,
    );
  } else if (isPlainObject(additionalProperties)) {
    for (const name of additional) {
      check(value[name], additionalProperties, [...where, name], [...at, "additionalProperties"], errors, skip);
    }
  }
  if (Array.isArray(required)) {
    for (const name of required) {
      if (typeof name === "string" && !(Object.hasOwn(value, name) && value[name] !== undefined)) {
        fail(["required"], `${canonicalJson(name)} is a required property`);
      }
    }
  }
  if (typeof minProperties === "number" && names.length < minProperties) {
    fail(["minProperties"], `${v()} has fewer than ${count(minProperties, "property", "properties")}`);
  }
  if (typeof maxProperties === "number" && names.length > maxProperties) {
    fail(["maxProperties"], `${v()} has more than ${count(maxProperties, "property", "properties")}`);
  }
}

function hasType(value: unknown, type: unknown): boolean {
  switch (type) {
    case "null":
      return value === null;
    case "boolean":
      return typeof value === "boolean";
    case "string":
      return typeof value === "string";
    case "number":
      return typeof value === "number";
    case "integer":
      return typeof value === "number" && Number.isInteger(value);
    case "array":
      return Array.isArray(value);
    case "object":
      return isPlainObject(value);
    default:
      // A type JSON Schema does not have: no value is of it.
      return false;
  }
}

/** Whether `value` is a whole multiple of `divisor`, exactly in decimal: both as their shortest
 * decimal numerals (as JSON writes them), scaled to integers by one power of ten. */
function isMultiple(value: number, divisor: number): boolean {
  if (!Number.isFinite(value) || !Number.isFinite(divisor)) {
    return false;
  }
  if (Number.isInteger(value) && Number.isInteger(divisor)) {
    return value % divisor === 0;
  }
  const a = decimal(value);
  const b = decimal(divisor);
  const exponent = Math.min(a.exponent, b.exponent);
  const scaled = a.digits * 10n ** BigInt(a.exponent - exponent);
  const by = b.digits * 10n ** BigInt(b.exponent - exponent);
  return by !== 0n && scaled % by === 0n;
}

/** A finite number as `digits × 10^exponent`, from its shortest decimal numeral. */
function decimal(n: number): { digits: bigint; exponent: number } {
  const [mantissa = "0", power = "0"] = String(Math.abs(n)).split("e");
  const [whole = "0", fraction = ""] = mantissa.split(".");
  return { digits: BigInt(`${whole}${fraction}`), exponent: Number(power) - fraction.length };
}

/** Whether the host's check can read `pattern` (module comment). */
export function isPattern(pattern: string): boolean {
  return regex(pattern) !== null;
}

/** `<n> <noun>`, the noun plural unless n is 1. */
function count(n: number, one: string, many: string): string {
  return `${canonicalJson(n)} ${n === 1 ? one : many}`;
}

/** A pattern as an ECMA-262 regular expression: with `u`, as JSON Schema reads it, else without
 * it (its Annex B syntax, which takes identity escapes such as `\-`); null for one that is
 * neither. Compiled patterns are kept. */
function regex(pattern: string): RegExp | null {
  if (COMPILED.has(pattern)) {
    return COMPILED.get(pattern) ?? null;
  }
  let expression: RegExp | null = null;
  for (const flags of ["u", ""]) {
    try {
      expression = new RegExp(pattern, flags);
      break;
    } catch {
      // the next reading
    }
  }
  COMPILED.set(pattern, expression);
  return expression;
}

const COMPILED = new Map<string, RegExp | null>();

/** Whether two JSON values are equal as JSON Schema compares them (member order aside). */
function equal(a: unknown, b: unknown): boolean {
  return canonicalJson(a) === canonicalJson(b);
}

/** A JSON pointer of segments. */
function pointer(segments: readonly Segment[]): string {
  return segments.map((segment) => `/${pointerToken(String(segment))}`).join("");
}

/** §6.2's order of places and of keyword paths. */
function compareSegments(a: readonly Segment[], b: readonly Segment[]): number {
  for (let index = 0; index < Math.min(a.length, b.length); index += 1) {
    const left = a[index] as Segment;
    const right = b[index] as Segment;
    if (left === right) {
      continue;
    }
    if (typeof left === "number" && typeof right === "number") {
      return left - right;
    }
    const x = String(left);
    const y = String(right);
    return x < y ? -1 : 1;
  }
  return a.length - b.length;
}

/**
 * A JSON value as canonical JSON (`spec/identity.md` §2, RFC 8785): no whitespace, members sorted
 * by their UTF-16 code units, numbers as ECMAScript writes them. An object member whose value is
 * `undefined` is left out.
 */
export function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map((item) => canonicalJson(item)).join(",")}]`;
  }
  if (isPlainObject(value)) {
    const members = Object.keys(value)
      .filter((key) => value[key] !== undefined)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`);
    return `{${members.join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}
