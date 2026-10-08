import { useEffect, useMemo, useSyncExternalStore } from "react";
import { ArtifactController, artifactDigests, canPreviewText, displayPortName, type Artifact, type CanvasPort, type RunNode } from "@grida/fx-web";
export { WorkflowCanvas } from "./canvas";

export function money(value: number | null): string {
  if (value === null) return "Not recorded";
  return new Intl.NumberFormat("en-US", {
    style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 5,
  }).format(value);
}

export function Status({ state }: { state: string }) {
  const colors: Record<string, string> = {
    succeeded: "bg-emerald-50 text-emerald-800 ring-emerald-200",
    failed: "bg-red-50 text-red-800 ring-red-200",
    running: "bg-blue-50 text-blue-800 ring-blue-200",
    cancelling: "bg-amber-50 text-amber-800 ring-amber-200",
    cancelled: "bg-amber-50 text-amber-800 ring-amber-200",
    interrupted: "bg-amber-50 text-amber-800 ring-amber-200",
    planned: "bg-blue-50 text-blue-800 ring-blue-200",
    maybe: "bg-amber-50 text-amber-800 ring-amber-200",
    done: "bg-emerald-50 text-emerald-800 ring-emerald-200",
    blocked: "bg-red-50 text-red-800 ring-red-200",
  };
  return <span className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] leading-3 font-medium ring-1 ring-inset ${colors[state] ?? "bg-zinc-100 text-zinc-600 ring-zinc-200"}`}>
    <span className="size-1 rounded-full bg-current" aria-hidden="true" />
    {state.replaceAll("_", " ")}
  </span>;
}

/** Full labels complement the compact, read-only SVG sockets. */
export function PortList({ ports }: { ports: CanvasPort[] }) {
  if (!ports.length) return <p className="text-xs text-zinc-400">None declared.</p>;
  return <ul>{ports.map((port) => <li key={`${port.kind}:${port.name}`} className="flex items-baseline justify-between gap-2 border-b border-zinc-100 py-1.5 last:border-b-0">
    <span className="flex min-w-0 flex-wrap items-baseline gap-x-1.5"><span className="break-all font-mono text-[11px] text-zinc-700">{displayPortName(port.name)}</span><span className="text-[9px] text-zinc-400">{port.kind === "parameter" ? "Parameter" : port.kind === "fact" ? "Fact" : port.kind === "value" ? "Workflow value" : "Artifact"}</span></span>
    <span className="max-w-[50%] shrink-0 break-all text-right font-mono text-[10px] text-zinc-500">{port.type}</span>
  </li>)}</ul>;
}

function bytes(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

export function ArtifactPreview({ artifact }: { artifact: Artifact }) {
  const controller = useMemo(() => new ArtifactController(), []);
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  useEffect(() => { void controller.load(artifact); return () => controller.dispose(); }, [controller, artifact]);
  const available = artifact.available && artifact.url !== null;
  const image = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/avif"].includes(artifact.kind);
  const audio = artifact.kind.startsWith("audio/");
  const video = artifact.kind.startsWith("video/");
  const text = ["json", "application/json", "text", "text/plain", "annotations"].includes(artifact.kind);
  return <div className="overflow-hidden rounded border border-zinc-200 bg-white">
    <div className="flex items-start justify-between gap-2 px-2.5 py-2">
      <div className="min-w-0 flex-1">
        <div title={artifact.name || artifact.digest} className="truncate text-[11px] font-medium text-zinc-800">{artifact.name || artifact.digest.slice(0, 12)}</div>
        <div className="mt-0.5 text-[10px] text-zinc-500">{artifact.kind} · {bytes(artifact.size)}</div>
      </div>
      {available && <a className="shrink-0 rounded border border-zinc-200 px-2 py-1 text-[10px] font-medium text-zinc-700 hover:bg-zinc-50" href={artifact.url!} target="_blank" rel="noopener noreferrer" title="Open artifact in a new tab">Open</a>}
    </div>
    {!available ? <p className="border-t border-zinc-200 bg-zinc-50 px-2.5 py-2 text-[11px] text-zinc-500">File unavailable in this run folder.</p>
      : state.error ? <p className="border-t border-zinc-200 px-2.5 py-2 text-[11px] text-zinc-500">Preview unavailable. Try opening the file.</p>
      : image ? <div className="preview-grid border-t border-zinc-200 p-2"><img className="mx-auto max-h-72 max-w-full rounded object-contain" src={artifact.url!} alt={artifact.name || "Workflow artifact"} loading="lazy" onError={controller.fail} /></div>
      : audio ? <div className="border-t border-zinc-200 p-2"><audio className="w-full" controls preload="metadata" src={artifact.url!} onError={controller.fail} /></div>
      : video ? <div className="border-t border-zinc-200 bg-zinc-100 p-2"><video className="max-h-72 w-full rounded" controls preload="metadata" src={artifact.url!} onError={controller.fail} /></div>
      : canPreviewText(artifact) ? <pre className="max-h-72 overflow-auto border-t border-zinc-200 bg-zinc-50 p-2 text-[11px] leading-4 whitespace-pre-wrap break-words">{state.text ?? "Loading preview…"}</pre>
      : <p className="border-t border-zinc-200 px-2.5 py-2 text-[11px] text-zinc-500">{text ? "This file is too large for an inline preview." : "Inline preview is not available for this file type."}</p>}
    <details className="border-t border-zinc-100 px-2.5 py-1.5 text-[10px] text-zinc-400">
      <summary className="cursor-pointer">Content digest</summary>
      <code className="mt-2 block break-all pb-1">{artifact.digest}</code>
    </details>
  </div>;
}

export function ValueList({ values, artifacts, empty }: {
  values: Record<string, unknown>;
  artifacts: Artifact[];
  empty: string;
}) {
  const entries = Object.entries(values);
  if (!entries.length) return <p className="rounded border border-dashed border-zinc-200 p-2 text-[11px] text-zinc-500">{empty}</p>;
  const byDigest = new Map(artifacts.map((item) => [item.digest, item]));
  return <div className="space-y-3">{entries.map(([name, value]) => {
    const digests = artifactDigests(value);
    return <section key={name}>
      <h4 className="mb-1.5 font-mono text-[11px] font-medium text-zinc-600">{name}</h4>
      {digests.length ? <div className="space-y-3">{digests.map((digest) => {
        const artifact = byDigest.get(digest);
        return artifact ? <ArtifactPreview key={digest} artifact={artifact} />
          : <p key={digest} className="rounded border border-dashed border-zinc-200 p-2 text-[11px] text-zinc-500">Referenced file is not in the artifact inventory.</p>;
      })}<details className="text-xs text-zinc-400"><summary className="cursor-pointer">Recorded value</summary><pre className="mt-2 max-h-60 overflow-auto whitespace-pre-wrap break-words">{JSON.stringify(value, null, 2)}</pre></details></div> : <pre className="max-h-60 overflow-auto rounded border border-zinc-200 bg-zinc-50 p-2 text-[11px] leading-4 whitespace-pre-wrap break-words">{JSON.stringify(value, null, 2)}</pre>}
    </section>;
  })}</div>;
}

export function Dependencies({ node, nodes, onSelect }: {
  node: RunNode;
  nodes: RunNode[];
  onSelect: (id: string) => void;
}) {
  return <div className="flex flex-wrap items-center gap-2 text-xs text-zinc-500">
    <span>Reads from</span>
    {node.reads.length ? node.reads.map((dependency) => {
      const match = nodes.find((item) => item.id === dependency || item.path === dependency);
      return match ? <button key={dependency} className="rounded-md border border-zinc-200 bg-white px-2 py-1 font-mono text-zinc-700 hover:border-zinc-400" onClick={() => onSelect(match.id)}>{dependency}</button>
        : <span key={dependency} className="rounded-md bg-zinc-100 px-2 py-1 font-mono">{dependency}</span>;
    }) : <span className="text-zinc-400">No recorded step dependencies</span>}
  </div>;
}
