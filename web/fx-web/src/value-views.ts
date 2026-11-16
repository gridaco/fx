import type { CanvasConnection, CanvasGraph, CanvasStep, GraphDocument } from "./graph";
import { cardPreview, recordedFileDigests } from "./graph";
import type { Artifact, ViewerRun } from "./index";

/** Engine-recorded data presentation; never an executable instance or an expression evaluator. */
export interface ValueView {
  id: string;
  scope_id: string | null;
  kind: "input" | "table" | "let" | "output";
  name: string;
  state: "known" | "pending" | "absent" | "unavailable" | "withheld";
  data?: unknown;
  context?: string;
  diagnostic?: string;
}
export type ValueEndpoint = { kind: "value"; value_id: string } | { kind: "instance"; instance_id: string; port: string; source_kind?: "output" | "fact" };
export interface ValueLink { source: ValueEndpoint; target: ValueEndpoint }
const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string";
const nonempty = (value: unknown): value is string => text(value) && value.length > 0;
const fields = (value: Record<string, unknown>, required: string[], optional: string[] = []) => required.every((key) => key in value) && Object.keys(value).every((key) => required.includes(key) || optional.includes(key));

function json(value: unknown, depth = 0): boolean {
  if (depth > 128) return false;
  if (value === null || typeof value === "string" || typeof value === "boolean") return true;
  if (typeof value === "number") return Number.isFinite(value);
  return Array.isArray(value) ? value.every((item) => json(item, depth + 1)) : record(value) && Object.values(value).every((item) => json(item, depth + 1));
}
/** Match the encoded-data bound, independently of wrapper or payload validation. */
function bounded(value: unknown, depth = 0): boolean {
  return depth <= 64 && (Array.isArray(value) ? value.every(item => bounded(item, depth + 1))
    : !record(value) || Object.values(value).every(item => bounded(item, depth + 1)));
}
/** Explicit encoded variants keep arbitrary JSON, including file-looking members, opaque. */
function encoded(value: unknown, depth = 0): boolean {
  if (depth > 128 || !record(value) || Object.keys(value).length !== 1) return false;
  if ("value" in value) return json(value.value, depth + 1);
  if (value.withheld === true || value.none === true) return true;
  if (record(value.file)) {
    const file = value.file;
    return fields(file, ["digest", "kind", "name", "size"], ["key"])
      && typeof file.digest === "string" && /^[a-f0-9]{64}$/.test(file.digest)
      && nonempty(file.kind) && text(file.name) && !/[\\\/\u0000-\u001f]/.test(file.name)
      && typeof file.size === "number" && Number.isSafeInteger(file.size) && file.size >= 0
      && (file.key === undefined || text(file.key));
  }
  if (record(value.object)) return Object.values(value.object).every((item) => encoded(item, depth + 1));
  if (Array.isArray(value.list)) return value.list.every((item) => encoded(item, depth + 1));
  return Array.isArray(value.collection) && value.collection.every((item) => Array.isArray(item) && item.length === 2 && text(item[0]) && encoded(item[1], depth + 1));
}

export function isValueViews(value: unknown): value is ValueView[] {
  return Array.isArray(value) && value.every((item) => record(item)
    && fields(item, ["id", "scope_id", "kind", "name", "state"], ["data", "context", "diagnostic"])
    && text(item.id) && /^value:[a-f0-9]{64}$/.test(item.id) && (item.scope_id === null || nonempty(item.scope_id))
    && ["input", "table", "let", "output"].includes(String(item.kind)) && nonempty(item.name)
    && ["known", "pending", "absent", "unavailable", "withheld"].includes(String(item.state))
    && (item.state === "known" ? bounded(item.data) && encoded(item.data) : !("data" in item))
    && (item.context === undefined || text(item.context) && Array.from(item.context).length <= 256) && (item.diagnostic === undefined || text(item.diagnostic)))
    && new Set(value.map((item) => item.id)).size === value.length;
}
function endpoint(value: unknown): value is ValueEndpoint {
  return record(value) && (value.kind === "value"
    ? fields(value, ["kind", "value_id"]) && nonempty(value.value_id)
    : value.kind === "instance" && fields(value, ["kind", "instance_id", "port"], ["source_kind"])
      && nonempty(value.instance_id) && text(value.port) && (value.source_kind === undefined || value.source_kind === "output" || value.source_kind === "fact"));
}
export function isValueLinks(value: unknown): value is ValueLink[] {
  return Array.isArray(value) && value.every((link) => record(link) && fields(link, ["source", "target"]) && endpoint(link.source) && endpoint(link.target));
}

/** Typed metadata can name an unavailable file without claiming that bytes exist. */
export function valueArtifacts(data: unknown, inventory: ReadonlyMap<string, Artifact>): Map<string, Artifact> {
  const artifacts = new Map(inventory);
  const visit = (value: unknown) => {
    if (!record(value) || "value" in value) return;
    if (record(value.file) && typeof value.file.digest === "string" && typeof value.file.kind === "string" && typeof value.file.name === "string" && typeof value.file.size === "number") {
      if (!artifacts.has(value.file.digest)) artifacts.set(value.file.digest, { digest: value.file.digest, kind: value.file.kind, name: value.file.name, size: value.file.size, available: false, url: null });
    } else if (record(value.object)) Object.values(value.object).forEach(visit);
    else if (Array.isArray(value.list)) value.list.forEach(visit);
    else if (Array.isArray(value.collection)) value.collection.forEach((item) => { if (Array.isArray(item) && item.length === 2) visit(item[1]); });
    else Object.values(value).forEach(visit);
  };
  visit(data);
  return artifacts;
}

/** Adapt presentation-only typed objects to the existing recorded-value inspection renderer. */
export function valueInspectionData(data: unknown): unknown {
  if (!record(data)) return data;
  if (record(data.object) && Object.keys(data).length === 1) return Object.fromEntries(Object.entries(data.object).map(([name, value]) => [name, valueInspectionData(value)]));
  if (Array.isArray(data.list)) return { list: data.list.map(valueInspectionData) };
  if (Array.isArray(data.collection)) return { collection: data.collection.map((entry) => Array.isArray(entry) && entry.length === 2 ? [entry[0], valueInspectionData(entry[1])] : entry) };
  return data;
}

/** Only explicit value wrappers become plain JSON; file-shaped authored objects stay opaque. */
export function valueText(value: unknown): string {
  if (record(value)) {
    if (Object.keys(value).length === 1 && "value" in value) return typeof value.value === "string" ? value.value : JSON.stringify(value.value);
    if (value.withheld === true) return "Withheld by the author";
    if (record(value.file) && typeof value.file.digest === "string") return `${String(value.file.name || "File")} · ${String(value.file.kind || "file")}`;
    if (typeof value.file === "string") return `file ${value.file.slice(0, 12)}`;
    if (value.none === true) return "No value recorded";
    if (value.missing === true) return "Absent";
    if (value.failed !== undefined) return "Upstream result unavailable";
    if (value.pending !== undefined) return "Waiting for an upstream result";
    if (record(value.object)) return Object.entries(value.object).slice(0, 4).map(([name, item]) => `${name}: ${valueText(item)}`).join(" · ");
    if (Array.isArray(value.list)) return `${value.list.length} items · ${value.list.slice(0, 3).map(valueText).join(", ")}${value.list.length > 3 ? ", …" : ""}`;
    if (Array.isArray(value.collection)) return `${value.collection.length} items · ${value.collection.slice(0, 3).map((item) => Array.isArray(item) ? String(item[0]) : "").join(", ")}${value.collection.length > 3 ? ", …" : ""}`;
  }
  return typeof value === "string" ? value : JSON.stringify(value) ?? "Not recorded";
}
export function valueStateLabel(value: ValueView): string {
  if (value.state !== "known") return ({ pending: "Waiting for an upstream result", absent: "Absent", unavailable: "Value unavailable", withheld: "Withheld by the author" })[value.state];
  if (record(value.data) && record(value.data.value)) {
    return Object.entries(value.data.value).map(([name, item]) => `${name}: ${typeof item === "string" ? item : JSON.stringify(item)}`).join("\n") || "{}";
  }
  return valueText(value.data);
}
export function valueKindLabel(kind: ValueView["kind"]): string {
  return { input: "Input", table: "Constant", let: "Expression", output: "Output" }[kind];
}
/** The small canvas excerpt has stable bounds, independent of media decoding or DOM measurement. */
export function valuePreviewLines(value: string, width = 43, maximum = 5): { lines: string[]; more: boolean } {
  const lines: string[] = [];
  const source = value.replaceAll("\t", "  ").split(/\r?\n/);
  let more = false;
  for (const line of source) {
    const chars = [...line];
    do {
      if (lines.length >= maximum) { more = true; break; }
      const space = chars.length > width ? chars.slice(0, width + 1).lastIndexOf(" ") : -1;
      const end = space >= width / 2 ? space : width;
      lines.push(chars.splice(0, end).join(""));
      if (space === end && chars[0] === " ") chars.shift();
    } while (chars.length);
    if (more) break;
  }
  if (more && lines.length) lines[lines.length - 1] = lines[lines.length - 1].replace(/.{0,1}$/, "…");
  return { lines, more };
}

/** Normalize effective step values for display, never to infer identity, files or provenance. */
export function stepValueSummary(values: Record<string, unknown>, encoded = false): { name: string; text: string }[] {
  const rank = (name: string, value: unknown) => name === "prompt" || name === "text" ? 0 : value === null ? 3 : typeof value === "string" ? 1 : 2;
  return Object.entries(values).sort(([a, left], [b, right]) => rank(a, left) - rank(b, right) || a.localeCompare(b))
    .slice(0, 4).map(([name, value]) => ({ name, text: encoded ? valueText(value) : typeof value === "string" ? value : JSON.stringify(value) ?? "Not recorded" }));
}

/** Apply value ownership after scope projection, so a closed import never leaks its private data. */
export function projectValueViews(view: GraphDocument | ViewerRun, graph: CanvasGraph, active: string | null, resolve: (digest: string) => Artifact | null): CanvasGraph {
  if (!view.value_views?.length) return graph;
  const scopes = new Map((view.scopes ?? []).map((scope) => [scope.id, scope]));
  const visibleOwner = (id: string | null) => {
    let owner = id;
    const seen = new Set<string>();
    while (owner !== null && owner !== active) {
      if (seen.has(owner)) return false;
      seen.add(owner);
      const scope = scopes.get(owner);
      if (!scope || scope.kind === "workflow") return false;
      owner = scope.parent;
    }
    return owner === active;
  };
  const views = view.value_views.filter((value) => visibleOwner(value.scope_id));
  const nodes = graph.nodes.filter((node) => node.kind !== "boundary" || !views.some((value) => value.scope_id === active && value.kind === node.side));
  const originalIds = new Set(nodes.map((node) => node.id));
  const frames = (graph.frames ?? []).map((frame) => ({ ...frame, nodes: [...frame.nodes] }));
  const metadata = new Map<string, Artifact>();
  for (const value of views) for (const [digest, artifact] of valueArtifacts(value.data, new Map())) metadata.set(digest, artifact);
  const artifacts = (value: ValueView) => cardPreview({ [value.name]: value.data }, (digest) => {
    const artifact = resolve(digest) ?? metadata.get(digest);
    return artifact ? { media_type: artifact.kind, name: artifact.name, size: artifact.size, url: artifact.available ? artifact.url : null } : null;
  });
  for (const value of views) {
    const preview = value.state === "known" ? artifacts(value) : undefined;
    const fileKinds = recordedFileDigests(value.data).flatMap((digest) => (resolve(digest) ?? metadata.get(digest))?.kind ?? []);
    const port = { name: value.name, type: fileKinds[0] ?? (value.state === "known" ? "value" : value.state), kind: "value" as const };
    const frame = value.scope_id === active ? null : frames.find((frame) => frame.id === value.scope_id);
    nodes.push({ id: value.id, source_id: value.id, kind: "value", value_id: value.id, value, scope_id: value.scope_id ?? undefined,
      address: `value:${value.scope_id ?? "root"}:${value.kind}:${value.name}`, title: value.name, subtitle: valueKindLabel(value.kind), state: value.state, pending: false,
      side: value.kind === "output" ? "output" : "input", ...(value.context ? { key: value.context } : {}),
      ports: { inputs: value.kind === "input" || value.kind === "table" ? [] : [port], outputs: value.kind === "output" ? [] : [port], settings: [] },
      ...(preview ? { preview } : {}), ...(fileKinds.some((kind) => /^(image|video|model)(?:\/|$)/.test(kind)) ? { value_media: true } : {}) });
    if (frame) frame.nodes.push(value.id);
  }
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const allValues = new Map((view.value_views ?? []).map((value) => [value.id, value]));
  const endpointNode = (endpoint: ValueEndpoint) => {
    if (endpoint.kind === "instance") return byId.get(`instance:${endpoint.instance_id}`);
    const direct = byId.get(endpoint.value_id);
    if (direct) return direct;
    const value = allValues.get(endpoint.value_id);
    // A closed workflow's recorded public interface can represent its own input/output,
    // never a private value from a deeper imported scope or an arbitrary internal alias.
    if (!value || value.kind !== "input" && value.kind !== "output" || value.scope_id === null) return undefined;
    const boundary = byId.get(value.scope_id);
    return boundary?.kind === "workflow" ? boundary : undefined;
  };
  const edges = graph.edges.filter((edge) => byId.has(edge.source) && byId.has(edge.target));
  for (const link of view.value_links ?? []) {
    const source = endpointNode(link.source), target = endpointNode(link.target);
    if (!source || !target || source.id === target.id) continue;
    const sourcePort = link.source.kind === "value" ? allValues.get(link.source.value_id)!.name : link.source.port;
    const targetPort = link.target.kind === "value" ? allValues.get(link.target.value_id)!.name : link.target.port;
    if (!source.ports || !target.ports) continue;
    if (link.source.kind === "instance" && !source.ports.outputs.some((port) => port.name === sourcePort)) {
      if (link.source.source_kind !== "fact") continue;
      source.ports.outputs = [...source.ports.outputs, { name: sourcePort, type: "fact", kind: "fact" }];
    }
    if (link.target.kind === "instance" && !target.ports.inputs.some((port) => port.name === targetPort)) {
      const setting = target.ports.settings.find((setting) => setting.name === targetPort);
      if (!setting) continue;
      target.ports.inputs = [...target.ports.inputs, { name: setting.name, type: setting.type, kind: "parameter" }];
      target.ports.settings = target.ports.settings.filter((setting) => setting.name !== targetPort);
    }
    if (!source.ports.outputs.some((port) => port.name === sourcePort) || !target.ports.inputs.some((port) => port.name === targetPort)) continue;
    const id = `value-link:${JSON.stringify(link)}`;
    edges.push({ id, source: source.id, target: target.id, kind: "data", kinds: ["value"], source_port: sourcePort, target_port: targetPort,
      ...(link.source.kind === "instance" && link.source.source_kind ? { source_kind: link.source.source_kind } : {}) } satisfies CanvasConnection);
  }
  // Recorded value links replace a flattened leaf connection for the same consumer parameter.
  const mediated = new Set(edges.filter((edge) => edge.kinds.includes("value") && originalIds.has(edge.target)).map((edge) => `${edge.target}\0${edge.target_port}`));
  return { ...graph, nodes, frames, edges: edges.filter((edge) => edge.kind !== "data" || edge.kinds.includes("value") || !mediated.has(`${edge.target}\0${edge.target_port}`)) };
}
