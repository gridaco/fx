import type { CanvasGraph, CanvasPort, CanvasStep, GraphDocument } from "./graph";
import type { Artifact, RunNode, ViewerRun } from "./index";
import { stepValueSummary, valueKindLabel, type ValueView } from "./value-views";

export interface PortImage { digest: string; name: string; kind: string; size: number; url: string | null; key?: string }
export interface PortInspection {
  state: "known" | "pending" | "absent" | "unavailable" | "withheld";
  text: string;
  description?: string;
  sources?: string[];
  images?: readonly PortImage[];
  image_count?: number;
}

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const own = (value: object, name: string) => Object.prototype.hasOwnProperty.call(value, name);
const labels = { pending: "Waiting for an upstream result", absent: "Absent", unavailable: "Value not recorded", withheld: "Withheld by the author" };
const unavailable = (state: Exclude<PortInspection["state"], "known">, text = labels[state]): PortInspection => ({ state, text });
const maximumText = 64 * 1024;
const imageKinds = new Set(["image/png", "image/jpeg", "image/webp", "image/gif", "image/avif"]);
const maximumImages = 12;

function bounded(text: string): string {
  if (text.length <= maximumText) return text;
  const end = /[\uD800-\uDBFF]/.test(text[maximumText - 1]) ? maximumText - 1 : maximumText;
  return `${text.slice(0, end)}\n…`;
}

/** A saved plan can seed success without execution; keep the existing evidence rule. */
export function recordedExecution(node: RunNode): boolean {
  return node.state === "running" || node.state === "waiting" || node.interrupted === true
    || node.started_at !== undefined || typeof node.duration_ms === "number" || typeof node.cache === "string"
    || Object.keys(node.outputs).length > 0;
}

/** Explicit JSON wrappers remain opaque, even when authored members resemble file/marker data. */
function fileMetadata(value: unknown, artifacts: ReadonlyMap<string, Artifact>): { name: string; kind: string; size: number } | undefined {
  if (typeof value === "string") {
    const artifact = artifacts.get(value);
    return artifact ? { name: artifact.name, kind: artifact.kind, size: artifact.size } : undefined;
  }
  return record(value) && typeof value.name === "string" && typeof value.kind === "string" && typeof value.size === "number"
    ? { name: value.name, kind: value.kind, size: value.size } : undefined;
}

function decoded(value: unknown, artifacts: ReadonlyMap<string, Artifact>, depth = 0): unknown {
  if (!record(value) || depth >= 64) return value;
  if (Object.keys(value).length === 1) {
    if (own(value, "value")) return value.value;
    if (Array.isArray(value.list)) return value.list.map((item) => decoded(item, artifacts, depth + 1));
    if (Array.isArray(value.collection)) return Object.fromEntries(value.collection.filter((item) => Array.isArray(item) && item.length === 2)
      .map(([name, item]) => [name, decoded(item, artifacts, depth + 1)]));
    if (record(value.object)) return Object.fromEntries(Object.entries(value.object).map(([name, item]) => [name, decoded(item, artifacts, depth + 1)]));
    if (own(value, "file")) return fileMetadata(value.file, artifacts) ?? "File metadata not recorded";
    if (value.withheld === true) return "Withheld by the author";
    if (value.missing === true) return "Absent";
    if (value.none === true) return "No value recorded";
    if (own(value, "pending")) return "Waiting for an upstream result";
    if (own(value, "failed")) return "Upstream result unavailable";
  }
  return Object.fromEntries(Object.entries(value).map(([name, item]) => [name, decoded(item, artifacts, depth + 1)]));
}

function inspectText(value: unknown, encoded: boolean, artifacts: ReadonlyMap<string, Artifact>): PortInspection {
  if (encoded && record(value) && Object.keys(value).length === 1) {
    if (value.withheld === true) return unavailable("withheld");
    if (value.missing === true) return unavailable("absent");
    if (value.none === true) return unavailable("unavailable");
    if (own(value, "pending")) return unavailable("pending");
    if (own(value, "failed")) return unavailable("unavailable", "Upstream result unavailable");
    if (own(value, "file")) {
      const file = fileMetadata(value.file, artifacts);
      return file ? { state: "known", text: bounded(`${file.name}\n${file.kind} · ${file.size} bytes`) }
        : unavailable("unavailable", "File metadata not recorded");
    }
  }
  const plain = encoded ? decoded(value, artifacts) : value;
  return { state: "known", text: bounded(typeof plain === "string" ? plain : JSON.stringify(plain, null, 2) ?? "Value not recorded") };
}

/** Only typed recorded variants identify images. Opaque JSON cannot introduce files or URLs. */
function inspect(value: unknown, encoded: boolean, artifacts: ReadonlyMap<string, Artifact>, resolve: (digest: string) => Artifact | null): PortInspection {
  const inspection = inspectText(value, encoded, artifacts);
  if (!encoded) return inspection;
  const images: PortImage[] = [];
  let count = 0;
  const visit = (item: unknown, key?: string, depth = 0) => {
    if (!record(item) || depth >= 64 || Object.keys(item).length !== 1 || own(item, "value")) return;
    if (own(item, "file")) {
      const file = item.file;
      const digest = typeof file === "string" ? file : record(file) && typeof file.digest === "string" ? file.digest : null;
      if (!digest || !/^[a-f0-9]{64}$/.test(digest)) return;
      const metadata = artifacts.get(digest) ?? fileMetadata(file, artifacts);
      if (!metadata || !imageKinds.has(metadata.kind)) return;
      count++;
      if (images.length >= maximumImages) return;
      const artifact = resolve(digest);
      const url = artifact?.digest === digest && artifact.available ? artifact.url : null;
      const label = key ?? (record(file) && typeof file.key === "string" ? file.key : undefined);
      images.push({ digest, name: metadata.name, kind: metadata.kind, size: metadata.size, url, ...(label !== undefined ? { key: label } : {}) });
    } else if (Array.isArray(item.list)) item.list.forEach((child, at) => visit(child, key === undefined ? undefined : `${key}/${at}`, depth + 1));
    else if (Array.isArray(item.collection)) for (const entry of item.collection) {
      if (Array.isArray(entry) && entry.length === 2 && typeof entry[0] === "string") visit(entry[1], key === undefined ? entry[0] : `${key}/${entry[0]}`, depth + 1);
    }
    else if (record(item.object)) for (const [name, child] of Object.entries(item.object)) visit(child, key === undefined ? name : `${key}/${name}`, depth + 1);
  };
  visit(value);
  return count ? { ...inspection, images, image_count: count } : inspection;
}

/** Populate socket inspection from recorded values after scope/value projection has fixed the ports. */
export function projectPortValues(view: GraphDocument | ViewerRun, graph: CanvasGraph, resolveArtifact: (digest: string) => Artifact | null = () => null): CanvasGraph {
  const run = view.kind === "fx-viewer-run-v1";
  const artifacts = new Map((run ? view.artifacts : []).map((artifact) => [artifact.digest, artifact]));
  const records = new Map((run ? view.nodes : view.instances).map((node) => [node.id, node]));
  const scopes = new Map((view.scopes ?? []).map((scope) => [scope.id, scope]));
  const owners = new Map((view.scopes ?? []).flatMap((scope) => scope.nodes.map((id) => [id, scope.id] as const)));
  const workflowOwner = (id: string | null): string | null => {
    const seen = new Set<string>();
    while (id !== null && !seen.has(id)) {
      seen.add(id);
      const scope = scopes.get(id);
      if (!scope) return id;
      if (scope.kind === "workflow") return scope.id;
      id = scope.parent;
    }
    return id;
  };
  const values = new Map((view.value_views ?? []).map((value) => [value.id, value]));
  const visible = new Map(graph.nodes.map((node) => [node.source_id, node]));
  const sources = new Map<string, string[]>();
  for (const link of view.value_links ?? []) {
    if (link.target.kind !== "instance") continue;
    const target = link.target, owner = workflowOwner(owners.get(target.instance_id) ?? null);
    let label: string | undefined;
    if (link.source.kind === "value") {
      const value = values.get(link.source.value_id);
      if (value && workflowOwner(value.scope_id) === owner) label = `${valueKindLabel(value.kind)} ${value.name}${value.context ? ` · ${value.context}` : ""}`;
    } else {
      const source = visible.get(link.source.instance_id);
      if (source) label = `${source.title} · ${link.source.port}`;
    }
    if (!label) continue;
    const key = JSON.stringify([target.instance_id, target.port]), hints = sources.get(key) ?? [];
    if (!hints.includes(label)) hints.push(label);
    sources.set(key, hints);
  }
  const publicValue = (node: CanvasStep, direction: "input" | "output", name: string): PortInspection => {
    const kind = node.kind === "boundary" ? node.side : direction;
    const found = (view.value_views ?? []).filter((value) => value.scope_id === node.scope_id && value.kind === kind && value.name === name);
    if (found.length !== 1) return unavailable(!run && kind === "output" ? "pending" : "unavailable");
    const value: ValueView = found[0];
    return value.state === "known" ? inspect(value.data, true, artifacts, resolveArtifact) : unavailable(value.state);
  };
  return { ...graph, nodes: graph.nodes.map((node) => {
    if (!node.ports || node.value) return node;
    const raw = records.get(node.source_id), executed = run && raw !== undefined && recordedExecution(raw as RunNode);
    const ports = (direction: "input" | "output", list: CanvasPort[]) => list.map((port): CanvasPort => {
      let inspection: PortInspection;
      if (node.kind === "workflow" || node.kind === "boundary") inspection = publicValue(node, direction, port.name);
      else if (port.kind === "fact" || !raw) inspection = unavailable("unavailable");
      else if (direction === "output") {
        const output = run ? (raw as RunNode).outputs : undefined;
        inspection = output && own(output, port.name) ? inspect(output[port.name], true, artifacts, resolveArtifact)
          : unavailable(raw.state === "absent" || raw.state === "skipped" || run && executed && raw.state === "succeeded" ? "absent"
            : !run || ["pending", "running", "waiting"].includes(raw.state) ? "pending" : "unavailable");
      } else if (own(raw.with, port.name)) {
        const value = raw.with[port.name];
        const waiting = run ? [...raw.reads, ...(raw.bindings?.map((binding) => binding.source) ?? [])] : (raw as GraphDocument["instances"][number]).waiting_on;
        const pending = record(value) && Object.keys(value).length === 1 && Array.isArray(value.pending)
          && value.pending.length > 0 && value.pending.every((id) => typeof id === "string" && waiting.includes(id));
        const withheld = run && record(value) && Object.keys(value).length === 1 && value.withheld === true;
        inspection = pending ? unavailable("pending") : inspect(value, executed || withheld, artifacts, resolveArtifact);
      } else inspection = unavailable(raw.state === "absent" || raw.state === "skipped" || executed ? "absent" : "unavailable");
      if (direction === "input" && port.kind === "parameter" && raw) {
        const schema = run ? (raw as RunNode).ports?.params[port.name] : view.types?.[raw.uses ?? ""]?.ports?.params[port.name];
        const description = schema?.description;
        if (typeof description === "string" && description.trim()) inspection = { ...inspection, description: bounded(description) };
      }
      const hints = direction === "input" ? sources.get(JSON.stringify([node.source_id, port.name])) : undefined;
      if (hints?.length) inspection = { ...inspection, sources: hints.slice(0, 16).map(bounded).concat(hints.length > 16 ? [`+${hints.length - 16} more recorded sources`] : []) };
      return { ...port, inspection };
    });
    const inputs = ports("input", node.ports.inputs), outputs = ports("output", node.ports.outputs);
    if (!raw || node.kind === "workflow" || node.kind === "boundary") return { ...node, ports: { ...node.ports, inputs, outputs } };
    const names = new Set(inputs.map((port) => port.name));
    const remaining = Object.fromEntries(Object.entries(raw.with).filter(([name]) => !names.has(name)));
    return { ...node, ports: { ...node.ports, inputs, outputs }, summaries: stepValueSummary(remaining, executed), summary_count: Object.keys(remaining).length };
  }) };
}
