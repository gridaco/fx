import { useEffect, useMemo, useSyncExternalStore } from "react";
import { ArtifactController, canPreviewText, jsonShape, jsonSize, parseJson, plainJson, stateIcon, stateIconParts, type Artifact } from "@grida/fx-web";
export { WorkflowCanvas, type WorkflowCanvasHandle } from "./canvas";

export function money(value: number | null): string {
  if (value === null) return "Not recorded";
  return new Intl.NumberFormat("en-US", {
    style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 5,
  }).format(value);
}

/** A state's glyph: the same circle slot the canvas cards draw. */
export function StateIcon({ state, size = 12 }: { state: string; size?: number }) {
  const icon = stateIcon(state);
  return <svg className="fx-state-icon" data-icon={icon} width={size} height={size} viewBox="0 0 16 16" aria-hidden="true">
    <g className="fx-state-glyph">{stateIconParts(icon).map(({ tag: Tag, attributes: { class: className, ...rest } }, index) => <Tag key={index} className={String(className)} {...rest} />)}</g>
  </svg>;
}

/** A state's icon and word, untinted. */
export function StateLabel({ state }: { state: string }) {
  return <span className="inline-flex items-center gap-1 font-medium text-zinc-700">
    <StateIcon state={state} size={12} />
    {state.replaceAll("_", " ")}
  </span>;
}

export function Status({ state }: { state: string }) {
  const colors: Record<string, string> = {
    succeeded: "bg-emerald-50 text-emerald-800 ring-emerald-200",
    failed: "bg-red-50 text-red-800 ring-red-200",
    cancelling: "bg-amber-50 text-amber-800 ring-amber-200",
    cancelled: "bg-amber-50 text-amber-800 ring-amber-200",
    interrupted: "bg-amber-50 text-amber-800 ring-amber-200",
    planned: "bg-blue-50 text-blue-800 ring-blue-200",
    maybe: "bg-amber-50 text-amber-800 ring-amber-200",
    done: "bg-emerald-50 text-emerald-800 ring-emerald-200",
    blocked: "bg-red-50 text-red-800 ring-red-200",
  };
  return <span className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] leading-3 font-medium ring-1 ring-inset ${colors[state] ?? "bg-zinc-100 text-zinc-600 ring-zinc-200"}`}>
    <StateIcon state={state} size={10} />
    {state.replaceAll("_", " ")}
  </span>;
}

const imageKinds = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/avif"];
const jsonKinds = ["json", "application/json", "annotations"];

/** The file itself: a picture, a player or its text. Clicking a picture opens the file. */
export function ArtifactPreview({ artifact }: { artifact: Artifact }) {
  const controller = useMemo(() => new ArtifactController(), []);
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  useEffect(() => { void controller.load(artifact); return () => controller.dispose(); }, [controller, artifact]);
  const name = artifact.name || artifact.digest.slice(0, 12);
  if (!artifact.available || artifact.url === null) return <p className="fx-value-note">{name} · file unavailable</p>;
  const url = artifact.url;
  const open = <a className="fx-value-note hover:text-zinc-800" href={url} target="_blank" rel="noopener noreferrer">{name} ↗</a>;
  if (state.error) return open;
  if (imageKinds.includes(artifact.kind)) return <a href={url} target="_blank" rel="noopener noreferrer" title={`${name} · open in a new tab`} className="preview-grid block rounded border border-zinc-200 p-2">
    <img className="mx-auto max-h-64 max-w-full object-contain" src={url} alt={name} loading="lazy" onError={controller.fail} />
  </a>;
  if (artifact.kind.startsWith("audio/")) return <audio className="w-full" controls preload="metadata" src={url} onError={controller.fail} />;
  if (artifact.kind.startsWith("video/")) return <video className="max-h-64 w-full rounded" controls preload="metadata" src={url} onError={controller.fail} />;
  if (canPreviewText(artifact)) {
    // A JSON file reads as its value does: rows, one line, or folded.
    const json = jsonKinds.includes(artifact.kind) && state.text !== null ? parseJson(state.text) : undefined;
    return json !== undefined ? <Json value={json} /> : <pre className="fx-value-text">{state.text ?? "…"}</pre>;
  }
  return open;
}

const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);

function image(value: unknown, artifacts: Map<string, Artifact>) {
  const kind = record(value) && record(value.file) && typeof value.file.digest === "string" ? artifacts.get(value.file.digest)?.kind : undefined;
  return kind !== undefined && imageKinds.includes(kind);
}

/** A recorded value in its plain form: text, a file, or a list or collection of them. */
export function Value({ value, artifacts }: { value: unknown; artifacts: Map<string, Artifact> }) {
  if (!record(value)) return <Json value={plainJson(value) ?? value} />;
  if ("value" in value && Object.keys(value).length === 1) return <Json value={value.value} />;
  if (record(value.file) && typeof value.file.digest === "string") {
    const artifact = artifacts.get(value.file.digest);
    return artifact ? <ArtifactPreview artifact={artifact} /> : <p className="fx-value-note">File not in this run folder</p>;
  }
  if (typeof value.file === "string" && /^[a-f0-9]{64}$/.test(value.file)) return <p className="fx-value-note" title={value.file}>file {value.file.slice(0, 12)}</p>;
  const json = plainJson(value);
  if (json !== undefined) return <Json value={json} />;
  // Pictures read best as a grid; anything else, one under another.
  if (Array.isArray(value.list)) return <div className={value.list.every((item) => image(item, artifacts)) ? "fx-value-list" : "fx-value-stack"}>{value.list.map((item, index) => <Value key={index} value={item} artifacts={artifacts} />)}</div>;
  if (Array.isArray(value.collection)) {
    const entries = value.collection.filter((item): item is [string, unknown] => Array.isArray(item) && item.length === 2);
    // A collection of pictures reads best as a captioned grid; anything else as rows.
    return entries.every(([, item]) => image(item, artifacts))
      ? <div className="fx-value-list">{entries.map(([key, item]) => <figure key={key}><Value value={item} artifacts={artifacts} /><figcaption>{key}</figcaption></figure>)}</div>
      : <Values values={Object.fromEntries(entries)} artifacts={artifacts} />;
  }
  if (value.none === true) return <p className="fx-value-note">none</p>;
  if (value.missing === true) return <p className="fx-value-note">missing</p>;
  if ("pending" in value) return <p className="fx-value-note">waiting for an upstream result</p>;
  if (typeof value.failed === "string") return <p className="fx-value-note">upstream {value.failed} failed</p>;
  // An object that holds files: its members as named rows, each shown as what it is.
  return <Values values={value} artifacts={artifacts} />;
}

/** An ordinary JSON value: text as written, anything else on one line. */
function Plain({ value }: { value: unknown }) {
  return typeof value === "string" ? <p className="fx-value-text">{value}</p> : <p className="fx-value-text font-mono">{JSON.stringify(value)}</p>;
}

const text = (item: unknown) => typeof item === "string" ? item : JSON.stringify(item);

/** Ordinary JSON by its shape ([`jsonShape`]): folded JSON is formatted, under its size. */
export function Json({ value }: { value: unknown }) {
  switch (jsonShape(value)) {
    case "scalar": return <Plain value={value} />;
    case "line": return <p className="fx-value-text font-mono">{(value as unknown[]).map(text).join(", ") || "[]"}</p>;
    case "fields": return <dl className="fx-value-fields">{Object.entries(value as Record<string, unknown>).map(([name, item]) => <div key={name} className="contents"><dt>{name}</dt><dd>{text(item)}</dd></div>)}</dl>;
    default: return <details className="fx-value-json"><summary>{jsonSize(value)} · JSON</summary><pre><code>{JSON.stringify(value, null, 2)}</code></pre></details>;
  }
}

/** Named values as label/value rows. */
export function Values({ values, artifacts }: { values: Record<string, unknown>; artifacts: Map<string, Artifact> }) {
  return <dl className="fx-values">{Object.entries(values).map(([name, value]) => <div key={name}><dt>{name}</dt><dd><Value value={value} artifacts={artifacts} /></dd></div>)}</dl>;
}
