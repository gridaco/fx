import { useState } from "react";
import { ArtifactPreview } from "@grida/fx-react";
import type { Artifact, WaitingRequest } from "@grida/fx-web";

const record = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);

function fieldLabel(name: string, schema?: unknown) {
  if (record(schema) && typeof schema.title === "string" && schema.title.trim()) return schema.title;
  const words = name.replace(/_/g, " ").replace(/([a-z])([A-Z])/g, "$1 $2");
  return words ? words[0].toUpperCase() + words.slice(1) : "Unnamed field";
}

function fieldSchema(schema: unknown, name: string) {
  return record(schema) && record(schema.properties) && Object.hasOwn(schema.properties, name)
    ? schema.properties[name] : undefined;
}

type ValueProps = { value: unknown; schema?: unknown; artifacts: Map<string, Artifact>; encoded?: boolean };

function Entries({ entries, ordered, schema, artifacts, encoded }: Omit<ValueProps, "value"> & {
  entries: [string, unknown][]; ordered?: boolean;
}) {
  const [limit, setLimit] = useState(8);
  if (!entries.length) return <span className="text-zinc-500">{ordered ? "Empty list" : "No fields"}</span>;
  const rows = entries.slice(0, limit).map(([key, value]) => {
    const childSchema = ordered && record(schema) ? schema.items : fieldSchema(schema, key);
    const content = <ReviewValue value={value} schema={childSchema} artifacts={artifacts} encoded={encoded} />;
    return ordered ? <li key={key}>{content}</li> : <div key={key} className="min-w-0">
      <dt className="text-[11px] text-zinc-500">{fieldLabel(key, childSchema)}</dt>
      <dd>{content}</dd>
    </div>;
  });
  return <div className="grid min-w-0 gap-1">{ordered
    ? <ol className="list-decimal space-y-1 pl-5">{rows}</ol>
    : <dl className="grid gap-1.5">{rows}</dl>}
    {entries.length > limit && <button type="button" className="w-fit text-[11px] underline"
      onClick={() => setLimit(limit + 8)}>Show {Math.min(8, entries.length - limit)} more</button>}
  </div>;
}

/** Only engine-encoded reviewed values can refer to artifacts. A response is always ordinary data. */
export function ReviewValue({ value, schema, artifacts, encoded = false }: ValueProps) {
  if (encoded && record(value)) {
    if (Object.hasOwn(value, "value")) return <ReviewValue value={value.value} schema={schema} artifacts={artifacts} />;
    if (record(value.file) && typeof value.file.digest === "string") {
      const artifact = artifacts.get(value.file.digest);
      return artifact ? <ArtifactPreview artifact={artifact} renderJson={data => <ReviewValue value={data} artifacts={artifacts} />} />
        : <span className="text-zinc-500">File not in this run folder</span>;
    }
    if (record(value.object)) return <Entries entries={Object.entries(value.object)} artifacts={artifacts} encoded />;
    if (Array.isArray(value.collection)) {
      const entries = value.collection.filter((item): item is [string, unknown] =>
        Array.isArray(item) && item.length === 2 && typeof item[0] === "string");
      return <Entries entries={entries} artifacts={artifacts} encoded />;
    }
    if (Array.isArray(value.list)) value = value.list;
  }
  if (Array.isArray(value)) return <Entries entries={Object.entries(value)} ordered schema={schema} artifacts={artifacts} encoded={encoded} />;
  if (record(value)) return <Entries entries={Object.entries(value)} schema={schema} artifacts={artifacts} />;
  const text = value === null ? "No value" : value === true ? "Yes" : value === false ? "No"
    : value === "" ? "Empty text" : String(value);
  return <span className="whitespace-pre-wrap [overflow-wrap:anywhere]">{text}</span>;
}

export function WaitingAnswerSummary({ request, artifacts }: { request: WaitingRequest; artifacts: Map<string, Artifact> }) {
  if (request.state.status !== "answered") return null;
  const answer = request.state.answer;
  const field = request.choice_field;
  const key = field && record(answer) ? answer[field] : answer;
  if (!request.choices || typeof key !== "string" || !Object.hasOwn(request.choices, key)) {
    return <div className="mt-2"><ReviewValue value={answer} schema={request.schema} artifacts={artifacts} /></div>;
  }
  const label = field ? fieldLabel(field, fieldSchema(request.schema, field)) : "Selected option";
  const rest = field && record(answer) ? Object.entries(answer).filter(([name]) => name !== field) : [];
  return <div className="mt-2 grid gap-2">
    <figure aria-label={`${label}: ${key}`} className="min-w-0 rounded border border-zinc-200 p-2">
      <figcaption className="mb-1.5 flex items-baseline gap-2 [overflow-wrap:anywhere]">
        <span className="text-zinc-500">{label}</span><strong>{key}</strong>
      </figcaption>
      <ReviewValue value={request.choices[key]} artifacts={artifacts} encoded />
    </figure>
    {rest.length > 0 && <Entries entries={rest} schema={request.schema} artifacts={artifacts} />}
  </div>;
}
