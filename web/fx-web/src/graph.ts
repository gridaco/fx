import type { ViewerRun } from "./index";
import { validateViewerApiBase } from "./route";
import { isNodePorts, isPortBindings, parameterType, type NodePorts, type PortBinding } from "./ports";
import { isInterfaceBindings, isWorkflowScopes, projectScopes, type InterfaceBinding, type WorkflowScope } from "./scopes";
import { skipExplanation, type SkipExplanation } from "./skip";

export type PlanState = "planned" | "maybe" | "absent" | "blocked" | "failed" | "done";
export interface Price { low_usd: number; high_usd: number }
/** An instance's price; `capped` when one of its calls is on a capped route, which holds its cap. */
export interface InstancePrice extends Price { capped?: true }
/** A step's own scalar output names; expressions and cross-instance references are never evaluated. */
export interface PreviewDescriptor { video: string; poster?: string }
/** Safe configuration provenance for a model resolved to a concrete route. */
export interface ModelSelection {
  model: string;
  model_source: "step" | "planning_project" | "workflow_home";
  provider_preference: string;
  provider_source: "planning_project" | "workflow_home";
}
export interface GraphInstance {
  id: string; path: string; step: string; take: number[]; uses: string; type: string | null;
  with: Record<string, unknown>;
  routes: Record<string, { route: string; fingerprint: string }>;
  routing?: Record<string, ModelSelection>;
  state: PlanState; identity: string | null; phase: number; key: string | null;
  judges: string | null; judged_by: string[]; waiting_on: string[]; needs: string[]; reads: string[];
  price: InstancePrice; view?: boolean | string; reason?: string;
  bindings?: PortBinding[];
  interface_bindings?: InterfaceBinding[];
  preview?: PreviewDescriptor;
}
export interface PendingRepeat { path: string; step?: string; max: number; phase: number; high_usd: number; waiting_on?: string[] }
export interface GraphDocument {
  kind: "fx-graph-v1";
  workflow: { id: string; title: string; description?: string; file?: string };
  instances: GraphInstance[];
  pending: PendingRepeat[];
  estimate: Price & { ceiling_usd: number | null };
  problems?: { where: string; message: string }[];
  plan?: string;
  inputs?: Record<string, unknown>;
  stand_in?: true;
  takes_file?: string;
  view_origins?: string[];
  steps?: Record<string, { title?: string | null; description?: string | null; uses?: string | null; view?: boolean | string; order?: number; preview?: PreviewDescriptor }>;
  types?: Record<string, { identity: string; source?: { files: Record<string, string>; resources: Record<string, string> }; ports?: NodePorts }>;
  scopes?: WorkflowScope[];
}

const states = new Set<PlanState>(["planned", "maybe", "absent", "blocked", "failed", "done"]);
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const isString = (value: unknown): value is string => typeof value === "string";
const isNullableString = (value: unknown) => value === null || isString(value);
const isStrings = (value: unknown): value is string[] => Array.isArray(value) && value.every(isString);
const isAmount = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value) && value >= 0;
const isPositiveInteger = (value: unknown) => typeof value === "number" && Number.isInteger(value) && value >= 1;
const isCount = (value: unknown) => typeof value === "number" && Number.isInteger(value) && value >= 0;
const isDistinctStrings = (value: unknown) => isStrings(value) && new Set(value).size === value.length;
const isDigest = (value: unknown) => isString(value) && /^[a-f0-9]{64}$/.test(value);
const isView = (value: unknown) => isString(value) || typeof value === "boolean";
const isRelative = (value: string) => value.split("/").every((part) => part !== "" && part !== "." && part !== "..");

export function isPreviewDescriptor(value: unknown): value is PreviewDescriptor {
  const name = (item: unknown) => isString(item) && /^[a-z][a-z0-9_]*$/.test(item);
  return isRecord(value) && fields(value, ["video"], ["poster"]) && name(value.video) && optional(value, "poster", name);
}

function fields(value: Record<string, unknown>, required: string[], optional: string[] = []) {
  return required.every((name) => name in value) && Object.keys(value).every((name) => required.includes(name) || optional.includes(name));
}
function optional(value: Record<string, unknown>, name: string, validate: (item: unknown) => boolean) {
  return !(name in value) || validate(value[name]);
}
function price(value: unknown): value is InstancePrice {
  return isRecord(value) && fields(value, ["low_usd", "high_usd"], ["capped"]) && isAmount(value.low_usd) && isAmount(value.high_usd)
    && optional(value, "capped", (item) => item === true);
}

/** "est. $0.40" or "est. $0.40 – $1.20"; "up to $1.50" when a call is capped, since only its
 * worst case is known; `free` for a price of nothing. `money` formats an amount of dollars. */
export function priceLabel(price: InstancePrice, money: (usd: number) => string, free = ""): string {
  if (price.capped) return `up to ${money(price.high_usd)}`;
  if (price.high_usd === 0) return free;
  return `est. ${money(price.low_usd)}${price.high_usd === price.low_usd ? "" : ` – ${money(price.high_usd)}`}`;
}
function withValue(value: unknown, depth = 0): boolean {
  if (depth > 512) return false;
  if (value === null || isString(value) || typeof value === "boolean") return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.every((item) => withValue(item, depth + 1));
  if (!isRecord(value)) return false;
  if (Object.keys(value).length === 1 && "pending" in value) {
    return isStrings(value.pending) && new Set(value.pending).size === value.pending.length;
  }
  return Object.values(value).every((item) => withValue(item, depth + 1));
}
function instance(value: unknown): value is GraphInstance {
  if (!isRecord(value) || !fields(value, ["id", "path", "step", "take", "uses", "type", "with", "routes", "state", "identity", "phase", "key", "judges", "judged_by", "waiting_on", "needs", "reads", "price"], ["view", "reason", "bindings", "interface_bindings", "preview", "routing"])) return false;
  return [value.id, value.path, value.step, value.uses].every(isString)
    && Array.isArray(value.take) && value.take.length > 0 && value.take.every(isPositiveInteger)
    && isNullableString(value.type) && isRecord(value.with) && Object.values(value.with).every((item) => withValue(item))
    && isRecord(value.routes) && Object.entries(value.routes).every(([name, route]) => /^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*$/.test(name)
      && isRecord(route) && fields(route, ["route", "fingerprint"]) && isString(route.route)
      && /^.+@[a-z0-9._-]+$/.test(route.route) && isDigest(route.fingerprint))
    && optional(value, "routing", (items) => isRecord(items) && Object.entries(items).every(([name, item]) => /^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*$/.test(name)
      && isRecord(item) && fields(item, ["model", "model_source", "provider_preference", "provider_source"])
      && isString(item.model) && /^[^@\s]+$/.test(item.model)
      && isString(item.model_source) && ["step", "planning_project", "workflow_home"].includes(item.model_source)
      && isString(item.provider_preference) && /^[a-z0-9._-]+$/.test(item.provider_preference)
      && isString(item.provider_source) && ["planning_project", "workflow_home"].includes(item.provider_source)))
    && states.has(value.state as PlanState) && (value.identity === null || isDigest(value.identity))
    && isPositiveInteger(value.phase) && isNullableString(value.key) && isNullableString(value.judges)
    && [value.judged_by, value.waiting_on, value.needs, value.reads].every(isStrings)
    && price(value.price) && optional(value, "reason", isString) && optional(value, "view", isView)
    && optional(value, "bindings", isPortBindings) && optional(value, "interface_bindings", isInterfaceBindings)
    && optional(value, "preview", isPreviewDescriptor);
}
function pendingRepeat(value: unknown): value is PendingRepeat {
  return isRecord(value) && fields(value, ["path", "max", "phase", "high_usd"], ["step", "waiting_on"])
    && isString(value.path) && isPositiveInteger(value.max) && isPositiveInteger(value.phase) && isAmount(value.high_usd)
    && optional(value, "step", isString) && optional(value, "waiting_on", isDistinctStrings);
}
function typeEntry(value: unknown): boolean {
  if (!isRecord(value) || !fields(value, ["identity"], ["source", "ports"]) || !isString(value.identity)
    || !optional(value, "ports", isNodePorts)) return false;
  if (/^fx\/[^@]+@(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)$/.test(value.identity)) return !("source" in value);
  if (!/^[^./#][^#]*#[A-Za-z_][A-Za-z0-9_]*@(?:0|[1-9][0-9]*|source:[0-9a-f]{64})$/.test(value.identity)) return false;
  const source = value.source;
  return isRecord(source) && fields(source, ["files", "resources"])
    && isRecord(source.files) && Object.keys(source.files).length > 0 && isRecord(source.resources)
    && [source.files, source.resources].every((entries) => Object.entries(entries).every(([path, digest]) => isRelative(path) && isDigest(digest)));
}

/** Browser validation of the fields defined by spec/schemas/fx-graph-v1.schema.json. */
export function parseGraph(value: unknown): GraphDocument {
  if (!isRecord(value) || value.kind !== "fx-graph-v1") throw new Error("This viewer does not support the returned graph format.");
  const valid = fields(value, ["kind", "workflow", "instances", "pending", "estimate"], ["problems", "plan", "inputs", "stand_in", "takes_file", "view_origins", "steps", "types", "scopes"])
    && isRecord(value.workflow) && fields(value.workflow, ["id", "title"], ["description", "file"])
    && isString(value.workflow.id) && /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(value.workflow.id) && value.workflow.id.length <= 96
    && isString(value.workflow.title) && optional(value.workflow, "description", isString) && optional(value.workflow, "file", isString)
    && Array.isArray(value.instances) && value.instances.every(instance)
    && new Set(value.instances.map((item) => item.id)).size === value.instances.length
    && Array.isArray(value.pending) && value.pending.every(pendingRepeat)
    && new Set(value.pending.map((item) => item.path)).size === value.pending.length
    && isRecord(value.estimate) && fields(value.estimate, ["low_usd", "high_usd", "ceiling_usd"])
    && isAmount(value.estimate.low_usd) && isAmount(value.estimate.high_usd)
    && (value.estimate.ceiling_usd === null || isAmount(value.estimate.ceiling_usd))
    && optional(value, "problems", (items) => Array.isArray(items) && items.every((item) => isRecord(item)
      && fields(item, ["where", "message"]) && isString(item.where) && isString(item.message)))
    && optional(value, "plan", isDigest) && optional(value, "inputs", isRecord)
    && optional(value, "stand_in", (item) => item === true)
    && optional(value, "takes_file", (item) => isString(item) && isRelative(item))
    && optional(value, "view_origins", isStrings)
    && optional(value, "steps", (items) => isRecord(items) && Object.values(items).every((item) => isRecord(item)
      && fields(item, [], ["title", "description", "uses", "view", "order", "preview"])
      && ["title", "description", "uses"].every((key) => optional(item, key, isNullableString)) && optional(item, "view", isView)
      && optional(item, "order", isCount) && optional(item, "preview", isPreviewDescriptor)))
    && optional(value, "types", (items) => isRecord(items) && Object.entries(items).every(([key, item]) => key.length > 0 && typeEntry(item)))
    && optional(value, "scopes", isWorkflowScopes);
  if (!valid) throw new Error("The graph response does not match fx-graph-v1.");
  return value as unknown as GraphDocument;
}

/** The engine's cells and member order (spec/layout.md §6.11, fx-layout-report-v1). */
export interface LayoutReport {
  kind: "fx-layout-report-v1";
  file: string | null;
  revision: string | null;
  state: "applied" | "none" | "refused";
  cursor: string | null;
  diagnostics: { code: string; message: string; address?: string }[];
  cells: Record<string, { column: number; row: number; source: "automatic" | "authored" }>;
  order: string[];
}

export function parseLayoutReport(value: unknown): LayoutReport {
  const valid = isRecord(value) && value.kind === "fx-layout-report-v1"
    && isNullableString(value.file) && isNullableString(value.revision) && isNullableString(value.cursor)
    && ["applied", "none", "refused"].includes(value.state as string)
    && Array.isArray(value.diagnostics) && value.diagnostics.every((item) => isRecord(item) && isString(item.code) && isString(item.message))
    && isRecord(value.cells) && Object.values(value.cells).every((cell) => isRecord(cell) && isCount(cell.column) && isCount(cell.row)
      && (cell.source === "automatic" || cell.source === "authored"))
    && isStrings(value.order);
  if (!valid) throw new Error("The layout response does not match fx-layout-report-v1.");
  return value as unknown as LayoutReport;
}

export interface CanvasPoster { digest: string; url: string; label: string }
interface CanvasPreviewBase {
  digest: string; label: string;
  /** The step's distinct output files of the preview's kind. */
  count: number;
}
export interface CanvasImagePreview extends CanvasPreviewBase { kind: "image"; url: string }
export interface CanvasVideoPreview extends CanvasPreviewBase {
  kind: "video";
  url: string | null;
  poster?: CanvasPoster;
  unavailable_reason?: "not_included" | "unavailable";
}
export interface CanvasModelPreview extends CanvasPreviewBase {
  kind: "model";
  url: string | null;
  name: string;
  size: number;
  unavailable_reason?: "not_included" | "unavailable";
}
/** Authored posters remain distinct from native decoded frames and do not establish playability. */
export type CanvasPreview = CanvasImagePreview | CanvasVideoPreview | CanvasModelPreview;
export interface CanvasStep {
  id: string; source_id: string; title: string; subtitle: string; state: string; pending: boolean;
  /** Recorded evidence that the node executed; status alone does not prove this for skipped nodes. */
  executed?: true;
  /** When the step last started, UTC: a running card counts up from it, less `waited_ms`. */
  started_at?: string;
  /** How long the step's latest start waited for route slots, in waits that ended. */
  waited_ms?: number;
  /** A waiting step's route, and when its wait began (UTC): a waiting card counts up from it. */
  waiting_for?: string;
  waiting_since?: string;
  /** The recorded declaration path: the key of the layout report's cells. */
  address?: string;
  /** The recorded take and item key, which order and label a deck. */
  take?: number[];
  key?: string | null;
  kind?: "workflow" | "boundary";
  /** A boundary card's side of an opened imported workflow. */
  side?: "input" | "output";
  scope_id?: string;
  path?: string;
  child_count?: number;
  failure_count?: number;
  errors?: { id: string; title: string; message: string }[];
  /** Human-readable presentation of recorded skip facts, never inferred from status or wiring. */
  skip_explanation?: SkipExplanation;
  preview?: CanvasPreview;
  ports?: { inputs: CanvasPort[]; outputs: CanvasPort[]; settings: { name: string; type: string }[] };
}
export interface CanvasPort { name: string; type: string; kind: "artifact" | "parameter" | "fact" | "value" }
export interface CanvasConnection {
  id: string; source: string; target: string; kinds: string[];
  /** A runtime read by an executed consumer, excluding its inspection of its judged subject. */
  consumed?: true;
  kind?: "data" | "control" | "dependency";
  source_port?: string; target_port?: string; source_kind?: "output" | "fact";
}
export interface CanvasFrame { id: string; title: string; nodes: string[]; parent?: string | null; address?: string; take?: number[]; key?: string; state?: string; executed?: true }
export interface CanvasGraph { nodes: CanvasStep[]; edges: CanvasConnection[]; frames?: CanvasFrame[] }

/** The inert image kinds a card can draw directly; models are rendered from verified GLB bytes. */
const previewKinds: Record<string, "image"> = {
  "image/png": "image", "image/jpeg": "image", "image/webp": "image", "image/gif": "image", "image/avif": "image",
};
// Concrete recorded video kinds follow the FX type grammar. The browser decides whether
// it can decode a file; its container type alone cannot establish codec support.
const videoKind = /^video\/[a-z0-9.+-]+$/;
const compareText = (left: string, right: string) => left < right ? -1 : left > right ? 1 : 0;
const compareNamed = (left: { name: string; kind?: string }, right: { name: string; kind?: string }) =>
  compareText(left.name, right.name) || compareText(left.kind ?? "", right.kind ?? "");

/** Follow recorded FX value variants; ordinary JSON values remain opaque and are never read as wiring. */
export function recordedFileDigests(value: unknown): string[] {
  if (!isRecord(value)) return [];
  if (isRecord(value.file)) return isDigest(value.file.digest) ? [value.file.digest as string] : [];
  if (Array.isArray(value.list)) return value.list.flatMap(recordedFileDigests);
  if (Array.isArray(value.collection)) return value.collection.flatMap((item) =>
    Array.isArray(item) && item.length === 2 ? recordedFileDigests(item[1]) : []);
  return [];
}

/**
 * A card's preview from a step's output files, in output order: the first image, or without one
 * the first video, then a GLB. `resolve` gives the recorded inventory entry, or null to omit it.
 * A GLB needs its recorded size for byte verification; a missing GLB retains an explicit fallback.
 */
export function cardPreview(outputs: Record<string, unknown>, resolve: (digest: string) => { media_type: string; name: string; size?: number; url: string | null; unavailable_reason?: CanvasVideoPreview["unavailable_reason"] } | null, selection?: PreviewDescriptor): CanvasPreview | undefined {
  if (selection) {
    const scalar = (name: string) => {
      const value = outputs[name];
      return isRecord(value) && isRecord(value.file) && isDigest(value.file.digest) ? value.file.digest as string : null;
    };
    const digest = scalar(selection.video), video = digest && resolve(digest);
    if (!digest || !video || !videoKind.test(video.media_type)) return undefined;
    const posterDigest = selection.poster ? scalar(selection.poster) : null, poster = posterDigest ? resolve(posterDigest) : null;
    return {
      kind: "video", digest, url: video.url, label: `${selection.video}: ${video.name}`, count: 1,
      ...(!video.url ? { unavailable_reason: video.unavailable_reason ?? "unavailable" } : {}),
      ...(posterDigest && poster?.url && previewKinds[poster.media_type] === "image" ? { poster: { digest: posterDigest, url: poster.url, label: `${selection.poster}: ${poster.name}` } } : {}),
    };
  }
  const found = { image: new Map<string, { url: string; label: string }>(), video: new Map<string, { url: string; label: string }>() };
  const models = new Map<string, Omit<CanvasModelPreview, "kind" | "digest" | "count">>();
  for (const [port, value] of Object.entries(outputs)) for (const digest of recordedFileDigests(value)) {
    const file = resolve(digest);
    const kind = file && (videoKind.test(file.media_type) ? "video" : previewKinds[file.media_type]);
    if (file?.url && kind && !found[kind].has(digest)) found[kind].set(digest, { url: file.url, label: `${port}: ${file.name}` });
    if (file?.media_type === "model/gltf-binary" && typeof file.size === "number" && Number.isSafeInteger(file.size) && file.size >= 0 && !models.has(digest)) {
      models.set(digest, { url: file.url, name: file.name, size: file.size, label: `${port}: ${file.name}`,
        ...(!file.url ? { unavailable_reason: file.unavailable_reason ?? "unavailable" } : {}) });
    }
  }
  const kind = found.image.size ? "image" : found.video.size ? "video" : undefined;
  const first = kind && found[kind].entries().next().value;
  if (kind && first) return { kind, digest: first[0], ...first[1], count: found[kind].size };
  const availableModels = [...models].filter(([, model]) => model.url !== null);
  const candidates = availableModels.length ? availableModels : [...models];
  const model = candidates[0];
  return model ? { kind: "model", digest: model[0], ...model[1], count: candidates.length } : undefined;
}

/** Select recorded output media from the host's verified artifact inventory. */
function recordedPreview(node: ViewerRun["nodes"][number], artifacts: Map<string, ViewerRun["artifacts"][number]>, apiBase: string): CanvasPreview | undefined {
  return cardPreview(node.outputs, (digest) => {
    const artifact = artifacts.get(digest);
    return artifact ? { media_type: artifact.kind, name: artifact.name, size: artifact.size, url: artifact.available && artifact.url === `${apiBase}/artifacts/${digest}` ? artifact.url : null } : null;
  }, node.preview);
}

/** A saved plan can seed `succeeded` without a node event; execution needs its own evidence. */
function recordedExecution(node: ViewerRun["nodes"][number]): boolean {
  return node.state === "running" || node.state === "waiting" || node.interrupted === true
    || node.started_at !== undefined || typeof node.duration_ms === "number" || typeof node.cache === "string"
    || Object.keys(node.outputs).length > 0;
}

/** Project declared wiring for display; no expressions or execution decisions are evaluated. */
export function flatCanvasGraph(view: GraphDocument | ViewerRun, apiBase = "/api"): CanvasGraph {
  validateViewerApiBase(apiBase);
  const plan = view.kind === "fx-graph-v1";
  const artifacts = new Map(plan ? [] : view.artifacts.map((item) => [item.digest, item]));
  const source = plan ? view.instances : view.nodes;
  const nodes: CanvasStep[] = source.map((item) => {
    const preview = plan ? undefined : recordedPreview(item as ViewerRun["nodes"][number], artifacts, apiBase);
    const run = plan ? undefined : item as ViewerRun["nodes"][number];
    const started_at = run?.started_at;
    const declared = plan ? view.types?.[item.uses ?? ""]?.ports : (item as ViewerRun["nodes"][number]).ports;
    const boundParams = new Set([...(item.bindings ?? []), ...(item.interface_bindings ?? [])].map((binding) => binding.target_port));
    const ports: CanvasStep["ports"] = declared ? {
      inputs: [
        ...Object.entries(declared.inputs).map(([name, type]) => ({ name, type, kind: "artifact" as const })),
        ...Object.entries(declared.params).filter(([name]) => boundParams.has(name)).map(([name, schema]) => ({ name, type: parameterType(schema), kind: "parameter" as const })),
      ],
      outputs: Object.entries(declared.outputs).map(([name, type]) => ({ name, type, kind: "artifact" as const })),
      settings: Object.entries(declared.params).filter(([name]) => !boundParams.has(name)).map(([name, schema]) => ({ name, type: parameterType(schema) })),
    } : undefined;
    return {
      id: `instance:${item.id}`, source_id: item.id,
      address: plan ? (item as GraphInstance).step : (item as ViewerRun["nodes"][number]).step ?? item.path,
      ...(item.take ? { take: item.take } : {}), ...(item.key !== undefined ? { key: item.key } : {}),
      title: plan ? view.steps?.[(item as GraphInstance).step]?.title || item.path : (item as ViewerRun["nodes"][number]).title || item.path,
      subtitle: item.uses ?? "Type not recorded", state: item.state, pending: false,
      ...(run && recordedExecution(run) ? { executed: true as const } : {}),
      ...(run?.state === "skipped" ? { skip_explanation: skipExplanation(run.skip_reason, (view as ViewerRun).nodes) } : {}),
      ...(started_at ? { started_at } : {}),
      ...(run?.waited_ms !== undefined ? { waited_ms: run.waited_ms } : {}),
      ...(run?.waiting_for !== undefined ? { waiting_for: run.waiting_for } : {}),
      ...(run?.waiting_since !== undefined ? { waiting_since: run.waiting_since } : {}),
      ...(preview ? { preview } : {}), ...(ports ? { ports } : {}),
    };
  });
  const nodeBySource = new Map(nodes.map((node) => [node.source_id, node]));
  // Facts are explicitly named by resolved bindings, and use their own socket kind.
  for (const item of source) for (const binding of item.bindings ?? []) {
    const upstream = nodeBySource.get(binding.source);
    if (binding.source_kind === "fact" && upstream?.ports
      && !upstream.ports.outputs.some((port) => port.kind === "fact" && port.name === binding.source_port)) {
      upstream.ports.outputs.push({ name: binding.source_port, type: "fact", kind: "fact" });
    }
  }
  // JSON maps carry no presentation order. Sort the new display arrays, never the record.
  for (const node of nodes) if (node.ports) {
    node.ports.inputs.sort(compareNamed);
    node.ports.outputs.sort(compareNamed);
    node.ports.settings.sort(compareNamed);
  }
  if (plan) nodes.push(...view.pending.map((item) => ({
    id: `pending:${item.path}`, source_id: item.path, address: item.step ?? item.path, title: item.path,
    subtitle: `Up to ${item.max} items · phase ${item.phase}`, state: "unexpanded", pending: true,
  })));
  const byId = new Map(source.map((item) => [item.id, item]));
  const byPath = new Map<string, typeof source[number] | null>();
  for (const item of source) byPath.set(item.path, byPath.has(item.path) ? null : item);
  const upstreamOf = (id: string) => byId.get(id) ?? byPath.get(id) ?? undefined;
  const connections = new Map<string, CanvasConnection>();
  for (const item of source) {
    const to = `instance:${item.id}`;
    const readIds = new Set(item.reads.flatMap((id) => upstreamOf(id)?.id ?? []));
    const judged = item.judges ? upstreamOf(item.judges)?.id : undefined;
    // A legacy event may update reads while leaving a planned binding behind.
    const consumed = (id: string) => !plan && nodeBySource.get(item.id)?.executed === true && readIds.has(id) && judged !== id;
    const preciseSources = new Set<string>();
    for (const binding of item.bindings ?? []) {
      const upstream = nodeBySource.get(binding.source);
      const target = nodeBySource.get(item.id);
      const outputKind = binding.source_kind === "fact" ? "fact" : "artifact";
      if (!upstream || upstream.id === to
        || !upstream.ports?.outputs.some((port) => port.name === binding.source_port && port.kind === outputKind)
        || !target?.ports?.inputs.some((port) => port.name === binding.target_port)) continue;
      const id = JSON.stringify(["data", upstream.id, binding.source_kind, binding.source_port, to, binding.target_port]);
      connections.set(id, { id, source: upstream.id, target: to, kind: "data", kinds: ["reads"], source_port: binding.source_port, target_port: binding.target_port, source_kind: binding.source_kind,
        ...(consumed(binding.source) ? { consumed: true } : {}) });
      preciseSources.add(binding.source);
    }
    const add = (dependency: string, role: string, kind: "control" | "dependency") => {
      const upstream = upstreamOf(dependency);
      if (!upstream || upstream.id === item.id || (kind === "dependency" && preciseSources.has(upstream.id))) return;
      const from = `instance:${upstream.id}`;
      const control = connections.get(JSON.stringify(["control", from, to]));
      if (kind === "dependency" && control) {
        if (!control.kinds.includes(role)) control.kinds.push(role);
        if (role === "reads" && consumed(upstream.id)) control.consumed = true;
        return;
      }
      const id = JSON.stringify([kind, from, to]);
      const edge = connections.get(id) ?? { id, source: from, target: to, kind, kinds: [] };
      if (!edge.kinds.includes(role)) edge.kinds.push(role);
      if (role === "reads" && consumed(upstream.id)) edge.consumed = true;
      connections.set(id, edge);
    };
    item.needs?.forEach((id) => add(id, "needs", "control"));
    if (item.judges) add(item.judges, "judges", "control");
    item.reads.forEach((id) => add(id, "reads", "dependency"));
    if (plan) (item as GraphInstance).waiting_on.forEach((id) => add(id, "waiting", "dependency"));
  }
  return { nodes, edges: [...connections.values()] };
}

export function canvasGraph(view: GraphDocument | ViewerRun, scopeId: string | null = null, apiBase = "/api"): CanvasGraph {
  return projectScopes(view, flatCanvasGraph(view, apiBase), scopeId);
}
