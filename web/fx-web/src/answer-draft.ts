/** The form keeps incomplete input separate from the answer sent to FX. */
export type AnswerSchema = boolean | Record<string, unknown>;

export type AnswerKind = "string" | "number" | "integer" | "boolean" | "null" | "array" | "object";

export type AnswerDraft =
  | { kind: "string" | "number" | "integer"; text: string }
  | { kind: "boolean"; value?: boolean }
  | { kind: "null" }
  | { kind: "array"; items: (AnswerDraft | undefined)[] }
  | { kind: "object"; fields: AnswerField[] };

export interface AnswerField {
  name: string;
  value?: AnswerDraft;
  editableName?: boolean;
}

const kinds: readonly AnswerKind[] = ["string", "number", "boolean", "null", "array", "object", "integer"];
const jsonNumber = /^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$/;

export function answerSchema(value: unknown): AnswerSchema {
  if (typeof value === "boolean") return value;
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown> : true;
}

export function answerKinds(schema: Record<string, unknown>): readonly AnswerKind[] {
  const declared = Array.isArray(schema.type) ? schema.type : [schema.type];
  const options = kinds.filter(kind => declared.includes(kind));
  return options.length > 0 ? options : kinds.filter(kind => kind !== "integer");
}

/** Literal scalar enums can use a picker. Its option values preserve the JSON type. */
export function scalarEnumOptions(schema: Record<string, unknown>) {
  if (!Array.isArray(schema.enum) || !schema.enum.length) return undefined;
  const options: { label: string; draft: AnswerDraft; encoded: string }[] = [];
  for (const value of schema.enum) {
    let draft: AnswerDraft;
    let label: string;
    if (value === null) {
      draft = { kind: "null" };
      label = "No value";
    } else if (typeof value === "string") {
      draft = { kind: "string", text: value };
      label = value || "Empty text";
    } else if (typeof value === "boolean") {
      draft = { kind: "boolean", value };
      label = value ? "Yes" : "No";
    } else if (typeof value === "number") {
      draft = { kind: "number", text: String(value) };
      label = String(value);
    } else {
      return undefined;
    }
    options.push({ label, draft, encoded: JSON.stringify(value) });
  }
  // Distinguish, for example, the text "1" and the number 1 in a mixed enum.
  const mixed = options.some(option => option.draft.kind !== options[0].draft.kind);
  return mixed ? options.map(option => ({ ...option, label: `${option.label} (${option.draft.kind})` })) : options;
}

export function emptyAnswer(kind: AnswerKind, schema: Record<string, unknown> = {}): AnswerDraft {
  switch (kind) {
    case "array": return { kind, items: [] };
    case "object": {
      const fields: AnswerField[] = [];
      const properties = answerSchema(schema.properties);
      if (typeof properties === "object" && Array.isArray(schema.required)) {
        for (const name of schema.required) {
          if (typeof name !== "string" || !Object.hasOwn(properties, name)) continue;
          const child = answerSchema(properties[name]);
          const value = initialAnswer(child);
          if (value !== undefined) fields.push({ name, value });
        }
      }
      return { kind, fields };
    }
    case "boolean":
    case "null": return { kind };
    default: return { kind, text: "" };
  }
}

/** Allow an empty list/object, and require explicit input for scalars. */
export function initialAnswer(schema: AnswerSchema): AnswerDraft | undefined {
  if (typeof schema === "boolean") return undefined;
  const [only, second] = answerKinds(schema);
  return !second && (only === "object" || only === "array") ? emptyAnswer(only, schema) : undefined;
}

/** Encode typed controls while preserving the exact spelling of a number for FX's JSON reader. */
export function encodeAnswer(draft: AnswerDraft | undefined): string | undefined {
  switch (draft?.kind) {
    case "string": return JSON.stringify(draft.text);
    case "number":
    case "integer": return jsonNumber.test(draft.text) ? draft.text : undefined;
    case "boolean": return draft.value === undefined ? undefined : String(draft.value);
    case "null": return "null";
    case "array": {
      const members = draft.items.map(encodeAnswer);
      return members.includes(undefined) ? undefined : `[${members.join(",")}]`;
    }
    case "object": {
      const members = [];
      const names = new Set<string>();
      for (const field of draft.fields) {
        const value = encodeAnswer(field.value);
        if (value === undefined || names.has(field.name)) return undefined;
        names.add(field.name);
        members.push(`${JSON.stringify(field.name)}:${value}`);
      }
      return `{${members.join(",")}}`;
    }
    case undefined: return undefined;
  }
}
