// A JSON Schema (draft 2020-12) validator for the keywords spec/schemas/fx-node-protocol-v1 uses,
// so the host's tests hold every message it writes to the protocol schema without a dependency.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { repoRoot } from "./support.js";

type Schema = boolean | { readonly [keyword: string]: unknown };

/** The protocol schema, read from spec/. */
export const PROTOCOL_SCHEMA = JSON.parse(
  readFileSync(join(repoRoot, "spec", "schemas", "fx-node-protocol-v1.schema.json"), "utf8"),
) as { readonly $defs: Record<string, Schema> };

/** Where `value` breaks `schema` (`$defs` from `root`): empty when it is valid. */
export function schemaErrors(value: unknown, schema: Schema, root: Schema = schema): string[] {
  const errors: string[] = [];
  validate(value, schema, root, "", errors);
  return errors;
}

/** Where `value` breaks the protocol schema's `$defs/<name>`, or the whole message schema. */
export function protocolErrors(value: unknown, name?: string): string[] {
  const schema: Schema = name === undefined ? (PROTOCOL_SCHEMA as Schema) : { $ref: `#/$defs/${name}` };
  return schemaErrors(value, schema, PROTOCOL_SCHEMA as Schema);
}

function typeOf(value: unknown): string {
  if (value === null) {
    return "null";
  }
  if (Array.isArray(value)) {
    return "array";
  }
  return typeof value;
}

function hasType(value: unknown, type: string): boolean {
  switch (type) {
    case "integer":
      return typeof value === "number" && Number.isInteger(value);
    case "number":
      return typeof value === "number";
    case "object":
      return typeOf(value) === "object";
    default:
      return typeOf(value) === type;
  }
}

function equal(a: unknown, b: unknown): boolean {
  if (a === b) {
    return true;
  }
  if (typeOf(a) !== typeOf(b) || typeof a !== "object" || a === null) {
    return false;
  }
  if (Array.isArray(a)) {
    const other = b as unknown[];
    return a.length === other.length && a.every((item, index) => equal(item, other[index]));
  }
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const keys = Object.keys(left);
  return keys.length === Object.keys(right).length && keys.every((key) => Object.hasOwn(right, key) && equal(left[key], right[key]));
}

function resolveRef(root: Schema, ref: string): Schema {
  if (!ref.startsWith("#")) {
    throw new Error(`only local references are read: ${ref}`);
  }
  let node: unknown = root;
  for (const token of ref.slice(1).split("/").slice(1)) {
    node = (node as Record<string, unknown>)[token.replaceAll("~1", "/").replaceAll("~0", "~")];
  }
  if (node === undefined) {
    throw new Error(`no schema at ${ref}`);
  }
  return node as Schema;
}

function validate(value: unknown, schema: Schema, root: Schema, at: string, errors: string[]): void {
  if (schema === true) {
    return;
  }
  if (schema === false) {
    errors.push(`${at || "/"}: no value is allowed here`);
    return;
  }
  const fail = (what: string) => errors.push(`${at || "/"}: ${what}`);
  const s = schema as Record<string, unknown>;
  if (typeof s.$ref === "string") {
    validate(value, resolveRef(root, s.$ref), root, at, errors);
  }
  if (s.type !== undefined) {
    const types = Array.isArray(s.type) ? (s.type as string[]) : [s.type as string];
    if (!types.some((type) => hasType(value, type))) {
      fail(`${JSON.stringify(value)?.slice(0, 80)} is not of type ${types.join(" or ")}`);
      return;
    }
  }
  if (s.const !== undefined && !equal(value, s.const)) {
    fail(`is not ${JSON.stringify(s.const)}`);
  }
  if (Array.isArray(s.enum) && !s.enum.some((item) => equal(value, item))) {
    fail(`${JSON.stringify(value)?.slice(0, 80)} is not one of ${JSON.stringify(s.enum)}`);
  }
  if (typeof value === "string") {
    if (typeof s.minLength === "number" && [...value].length < s.minLength) {
      fail(`is shorter than ${s.minLength}`);
    }
    if (typeof s.pattern === "string" && !new RegExp(s.pattern, "u").test(value)) {
      fail(`${JSON.stringify(value).slice(0, 80)} does not match ${s.pattern}`);
    }
  }
  if (typeof value === "number") {
    if (typeof s.minimum === "number" && value < s.minimum) {
      fail(`is less than ${s.minimum}`);
    }
    if (typeof s.maximum === "number" && value > s.maximum) {
      fail(`is more than ${s.maximum}`);
    }
    if (typeof s.exclusiveMinimum === "number" && value <= s.exclusiveMinimum) {
      fail(`is not more than ${s.exclusiveMinimum}`);
    }
  }
  if (Array.isArray(value)) {
    const prefix = Array.isArray(s.prefixItems) ? (s.prefixItems as Schema[]) : [];
    prefix.forEach((item, index) => {
      if (index < value.length) {
        validate(value[index], item, root, `${at}/${index}`, errors);
      }
    });
    if (s.items !== undefined) {
      for (let index = prefix.length; index < value.length; index += 1) {
        validate(value[index], s.items as Schema, root, `${at}/${index}`, errors);
      }
    }
    if (typeof s.minItems === "number" && value.length < s.minItems) {
      fail(`has fewer than ${s.minItems} items`);
    }
    if (typeof s.maxItems === "number" && value.length > s.maxItems) {
      fail(`has more than ${s.maxItems} items`);
    }
    if (s.uniqueItems === true && value.some((item, index) => value.findIndex((other) => equal(item, other)) !== index)) {
      fail("has repeated items");
    }
  }
  if (typeOf(value) === "object") {
    const object = value as Record<string, unknown>;
    const properties = (s.properties ?? {}) as Record<string, Schema>;
    for (const key of (s.required ?? []) as string[]) {
      if (!Object.hasOwn(object, key)) {
        fail(`misses ${key}`);
      }
    }
    for (const [key, item] of Object.entries(object)) {
      const where = `${at}/${key}`;
      if (Object.hasOwn(properties, key)) {
        validate(item, properties[key] as Schema, root, where, errors);
      } else if (s.additionalProperties !== undefined) {
        validate(item, s.additionalProperties as Schema, root, where, errors);
      }
      if (s.propertyNames !== undefined) {
        validate(key, s.propertyNames as Schema, root, `${where} (name)`, errors);
      }
    }
  }
  for (const part of (s.allOf ?? []) as Schema[]) {
    validate(value, part, root, at, errors);
  }
  if (Array.isArray(s.anyOf)) {
    if (!(s.anyOf as Schema[]).some((part) => schemaErrors(value, part, root).length === 0)) {
      fail("matches none of anyOf");
    }
  }
  if (Array.isArray(s.oneOf)) {
    const matched = (s.oneOf as Schema[]).filter((part) => schemaErrors(value, part, root).length === 0).length;
    if (matched !== 1) {
      const detail =
        matched === 0
          ? (s.oneOf as Schema[]).map((part) => schemaErrors(value, part, root).slice(0, 2).join("; ")).join(" | ")
          : `${matched} match`;
      fail(`matches ${matched} of oneOf, not one: ${detail}`);
    }
  }
  if (s.not !== undefined && schemaErrors(value, s.not as Schema, root).length === 0) {
    fail("matches what not refuses");
  }
  if (s.if !== undefined) {
    const holds = schemaErrors(value, s.if as Schema, root).length === 0;
    if (holds && s.then !== undefined) {
      validate(value, s.then as Schema, root, at, errors);
    }
    if (!holds && s.else !== undefined) {
      validate(value, s.else as Schema, root, at, errors);
    }
  }
}
