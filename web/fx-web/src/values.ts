/** Recorded values as the viewer shows them. */

const MARKERS = ["file", "collection", "none", "missing", "pending", "failed"];
const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * The ordinary JSON an encoded value stands for (`{"value": …}`, `{"list": […]}`, a plain
 * object), or `undefined` when it holds, at any depth, a file, a collection or a marker the
 * viewer shows otherwise.
 */
export function plainJson(value: unknown): unknown {
  if (Array.isArray(value)) {
    const items = value.map(plainJson);
    return items.includes(undefined) ? undefined : items;
  }
  if (!record(value)) return value;
  const keys = Object.keys(value);
  if (keys.length === 1 && keys[0] === "value") return value.value;
  if (keys.length === 1 && Array.isArray(value.list)) return plainJson(value.list);
  if (MARKERS.some((marker) => marker in value)) return undefined;
  const members = Object.entries(value).map(([name, item]) => [name, plainJson(item)] as const);
  return members.some(([, item]) => item === undefined) ? undefined : Object.fromEntries(members);
}

/** A JSON file's text as a value, or `undefined` when it does not parse. */
export function parseJson(text: string): unknown {
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
}

/** How a JSON value reads best: as text, one line, name/value rows, or folded JSON. */
export type JsonShape = "scalar" | "line" | "fields" | "folded";

const scalar = (value: unknown) => value === null || ["string", "number", "boolean"].includes(typeof value);

/** A short list of scalars on one line, an object of a few scalars as rows, anything deeper folded. */
export function jsonShape(value: unknown): JsonShape {
  if (scalar(value)) return "scalar";
  if (Array.isArray(value) && value.every(scalar) && value.length <= 16) return "line";
  if (record(value) && Object.values(value).every(scalar) && Object.keys(value).length <= 12) return "fields";
  return "folded";
}

/** What folded JSON says it holds: `9 items`, `4 fields`. */
export function jsonSize(value: unknown): string {
  if (Array.isArray(value)) return `${value.length} ${value.length === 1 ? "item" : "items"}`;
  const fields = record(value) ? Object.keys(value).length : 0;
  return `${fields} ${fields === 1 ? "field" : "fields"}`;
}
