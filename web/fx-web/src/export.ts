import { parseViewerRun, type ViewerRun } from "./index";
import { cardPreview, flatCanvasGraph, parseLayoutReport, type CanvasGraph, type LayoutReport } from "./graph";
import { projectScopes } from "./scopes";

export interface ExportArtifact {
  digest: string;
  media_type: string;
  name: string;
  size: number;
  path: string | null;
  omitted_reason: string | null;
}

/** A frozen inspection record. Resource locations are deliberately separate from the local API. */
export interface RunExport {
  kind: "fx-run-export-v1";
  producer: { version: string };
  source: { plan_sha256: string; event_count: number; events_sha256: string };
  disclosure: { values: boolean; diagnostics: boolean; artifacts: "media" };
  view: ViewerRun;
  layout: LayoutReport;
  artifacts: ExportArtifact[];
}

export interface PinnedRunExport { url: string; sha256: string }
export interface LoadedRunExport { manifest: RunExport; manifest_url: string }

const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const digest = (value: unknown): value is string => typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
const count = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const mediaExtensions: Record<string, string> = {
  "image/png": "png", "image/jpeg": "jpg", "image/webp": "webp", "image/gif": "gif", "image/avif": "avif",
  "video/mp4": "mp4", "video/webm": "webm", "audio/wav": "wav", "audio/mpeg": "mp3", "audio/ogg": "ogg", "audio/flac": "flac", "model/gltf-binary": "glb",
};

function refuse(message: string): never { throw new Error(`The recorded workflow is invalid: ${message}`); }

function known(value: Record<string, unknown>, keys: readonly string[]) { return Object.fromEntries(Object.entries(value).filter(([key]) => keys.includes(key))); }
const bindingFields = ["source", "source_port", "source_kind", "target_port"];
function bindings(value: unknown) { return Array.isArray(value) ? value.map((item) => record(item) ? known(item, bindingFields) : item) : value; }

/** Frozen v1 nested structure accepts future optional fields without widening local-host validators. */
function inspectionShape(value: unknown): unknown {
  if (!record(value)) return value;
  return {
    ...value,
    ...(Array.isArray(value.nodes) ? { nodes: value.nodes.map((item) => !record(item) ? item : {
      ...item,
      ...(record(item.ports) ? { ports: known(item.ports, ["inputs", "outputs", "params"]) } : {}),
      ...(item.bindings !== undefined ? { bindings: bindings(item.bindings) } : {}),
      ...(item.interface_bindings !== undefined ? { interface_bindings: bindings(item.interface_bindings) } : {}),
    }) } : {}),
    ...(Array.isArray(value.scopes) ? { scopes: value.scopes.map((item) => !record(item) ? item : {
      ...known(item, ["id", "parent", "kind", "path", "step", "take", "title", "source", "ports", "input_bindings", "output_bindings", "nodes", "pending"]),
      ...(record(item.ports) ? { ports: known(item.ports, ["inputs", "outputs"]) } : {}),
      input_bindings: bindings(item.input_bindings), output_bindings: bindings(item.output_bindings),
    }) } : {}),
  };
}

function validateValue(value: unknown, includeValues: boolean, inventory: Set<string>, depth = 0) {
  if (depth > 512) refuse("an encoded inspection value is too deeply nested.");
  if (!record(value)) {
    if (!includeValues) refuse("a withheld value was unexpectedly disclosed.");
    return;
  }
  if (value.withheld === true) return;
  if (record(value.file)) {
    const file = value.file;
    if (!record(file) || !digest(file.digest) || !inventory.has(file.digest) || typeof file.kind !== "string" || typeof file.name !== "string" || !count(file.size)) refuse("a file value is malformed or missing from the inventory.");
    return;
  }
  if (Array.isArray(value.list)) {
    for (const item of value.list) validateValue(item, includeValues, inventory, depth + 1);
    return;
  }
  if (Array.isArray(value.collection)) {
    for (const entry of value.collection) {
      if (!Array.isArray(entry) || entry.length !== 2 || typeof entry[0] !== "string") refuse("a recorded collection is malformed.");
      validateValue(entry[1], includeValues, inventory, depth + 1);
    }
    return;
  }
  if (!includeValues) refuse("a withheld value was unexpectedly disclosed.");
}

function pendingRepeat(item: unknown) {
  return record(item) && typeof item.path === "string" && count(item.max) && item.max >= 1 && count(item.phase) && item.phase >= 1
    && typeof item.high_usd === "number" && Number.isFinite(item.high_usd) && item.high_usd >= 0
    && (item.step === undefined || typeof item.step === "string")
    && (item.waiting_on === undefined || (Array.isArray(item.waiting_on) && item.waiting_on.every((id) => typeof id === "string") && new Set(item.waiting_on).size === item.waiting_on.length));
}

/** Only FX value variants contain artifact references; opaque JSON is never interpreted as wiring. */
export function exportFileDigests(value: unknown): string[] {
  if (!record(value)) return [];
  if (record(value.file)) return digest(value.file.digest) ? [value.file.digest] : [];
  if (Array.isArray(value.list)) return value.list.flatMap(exportFileDigests);
  if (Array.isArray(value.collection)) return value.collection.flatMap((entry) => Array.isArray(entry) && entry.length === 2 ? exportFileDigests(entry[1]) : []);
  return [];
}

function validateReferences(view: ViewerRun, layout: LayoutReport, artifacts: ExportArtifact[]) {
  const ids = new Set(view.nodes.map((node) => node.id));
  if (ids.size !== view.nodes.length) refuse("duplicate node identities.");
  const nodes = new Map(view.nodes.map((node) => [node.id, node]));
  const scopes = new Map((view.scopes ?? []).map((scope) => [scope.id, scope]));
  const pending = new Set((view.pending ?? []).map((item) => item.path));
  if (pending.size !== (view.pending ?? []).length) refuse("duplicate pending identities.");
  if ((view.pending ?? []).some((repeat) => (repeat.waiting_on ?? []).some((id) => !ids.has(id)))) refuse("a pending repeat dependency is missing.");
  for (const scope of scopes.values()) {
    if (scope.nodes.some((id) => !ids.has(id)) || scope.pending.some((id) => !pending.has(id))) refuse("a workflow scope names missing members.");
  }
  const checkBinding = (binding: { source: string; source_kind: string; source_port: string }) => {
    if (binding.source_kind === "scope_input" || binding.source_kind === "scope_output") {
      const scope = scopes.get(binding.source);
      if (!scope || (binding.source_kind === "scope_input" ? !(binding.source_port in scope.ports.inputs) : !scope.ports.outputs.includes(binding.source_port))) refuse("an interface binding names a missing workflow port.");
    } else {
      const source = nodes.get(binding.source);
      if (!source) refuse("a binding names a missing node.");
      if (binding.source_kind === "output" && source.ports && !(binding.source_port in source.ports.outputs)) refuse("a binding names a missing node output port.");
    }
  };
  for (const node of view.nodes) {
    if ([...node.reads, ...(node.needs ?? []), ...(node.judges ? [node.judges] : [])].some((id) => !ids.has(id))) refuse("a node dependency is missing.");
    for (const binding of [...(node.bindings ?? []), ...(node.interface_bindings ?? [])]) {
      checkBinding(binding);
      if (node.ports && !(binding.target_port in node.ports.inputs) && !(binding.target_port in node.ports.params)) refuse("a binding names a missing node input port.");
    }
  }
  for (const scope of scopes.values()) {
    for (const binding of scope.input_bindings) { checkBinding(binding); if (!(binding.target_port in scope.ports.inputs)) refuse("a workflow input binding names a missing alias."); }
    for (const binding of scope.output_bindings) { checkBinding(binding); if (!scope.ports.outputs.includes(binding.target_port)) refuse("a workflow output binding names a missing alias."); }
  }
  const inventory = new Set(artifacts.map((item) => item.digest));
  for (const values of [view.inputs, view.outputs, ...view.nodes.flatMap((node) => [node.with, node.outputs])]) {
    for (const value of Object.values(values)) if (exportFileDigests(value).some((id) => !inventory.has(id))) refuse("an artifact reference is missing from the inventory.");
  }
  const addresses = new Set([
    ...view.nodes.map((node) => node.step ?? node.path),
    ...(view.scopes ?? []).map((scope) => scope.step),
    ...(view.pending ?? []).map((item) => item.step ?? item.path),
  ]);
  // Cells preserve declared and previously seen slots, including slots without current members.
  if ([...addresses].some((address) => !(address in layout.cells))) refuse("layout cells do not cover the recorded topology.");
  const members = new Set([...ids, ...scopes.keys()]);
  // Terminal views retain unused planned takes as skipped; those never joined the member order.
  const unused = (node: ViewerRun["nodes"][number]) => node.state === "skipped" && node.cache === null && node.duration_ms === null && node.started_at === undefined && Object.keys(node.outputs).length === 0;
  const requiredMembers = [...view.nodes.filter((node) => !unused(node)).map((node) => node.id), ...scopes.keys()];
  if (new Set(layout.order).size !== layout.order.length || layout.order.some((id) => !members.has(id)) || requiredMembers.some((id) => !layout.order.includes(id))) refuse("layout order does not match the recorded topology.");
}

/** Static exports receive their own strict transport boundary; local URL validation is unchanged. */
export function parseRunExport(value: unknown): RunExport {
  if (!record(value) || value.kind !== "fx-run-export-v1") throw new Error("This website does not support the recorded workflow format.");
  if (!record(value.producer) || typeof value.producer.version !== "string"
    || !record(value.source) || !digest(value.source.plan_sha256) || !digest(value.source.events_sha256) || !count(value.source.event_count) || value.source.event_count < 1
    || !record(value.disclosure) || typeof value.disclosure.values !== "boolean" || typeof value.disclosure.diagnostics !== "boolean" || value.disclosure.artifacts !== "media"
    || !Array.isArray(value.artifacts)) refuse("required export metadata is missing.");
  const view = parseViewerRun(inspectionShape(value.view));
  if (view.artifacts.some((item) => item.url !== null)) refuse("local or remote artifact URLs are forbidden in the inspection record.");
  if (!["succeeded", "failed", "cancelled"].includes(view.state)) refuse("the recording is not terminal.");
  if (view.charged_usd !== null && view.charged_usd < 0) refuse("the recorded charge is invalid.");
  if (view.estimate !== null && (![view.estimate.low_usd, view.estimate.high_usd].every((amount) => Number.isFinite(amount) && amount >= 0))) refuse("the recorded estimate is invalid.");
  if ((view.scopes ?? []).some((scope) => scope.source !== null)) refuse("workflow source locations are forbidden in the inspection record.");
  for (const node of view.nodes) {
    if (!["pending", "succeeded", "failed", "skipped"].includes(node.state)
      || (node.take !== undefined && (!Array.isArray(node.take) || !node.take.length || !node.take.every((part) => Number.isSafeInteger(part) && part >= 1)))
      || (node.key !== undefined && node.key !== null && typeof node.key !== "string")
      || (node.duration_ms !== null && !count(node.duration_ms)) || !["hit", "miss", null].includes(node.cache)) refuse("a recorded node is malformed or still running.");
  }
  if (view.pending !== undefined && (!Array.isArray(view.pending) || !view.pending.every(pendingRepeat))) refuse("pending repeats are malformed.");
  const layout = parseLayoutReport(value.layout);
  if (layout.cursor !== null || layout.file !== null || layout.revision !== null) refuse("local layout coordinates are not portable.");
  if (Object.values(layout.cells).some((cell) => !count(cell.column) || !count(cell.row))) refuse("layout coordinates are invalid.");
  const seen = new Set<string>();
  for (const item of value.artifacts) {
    if (!record(item) || !digest(item.digest) || typeof item.media_type !== "string" || typeof item.name !== "string" || /[\\/\u0000-\u001f]/.test(item.name) || !count(item.size)
      || !(item.path === null || typeof item.path === "string") || !(item.omitted_reason === null || typeof item.omitted_reason === "string")) refuse("an artifact entry is malformed.");
    if (seen.has(item.digest)) refuse("duplicate artifact identities.");
    seen.add(item.digest);
    if (item.path !== null) {
      const extension = mediaExtensions[item.media_type];
      if (!extension || item.path !== `artifacts/${item.digest}.${extension}` || item.omitted_reason !== null) refuse("an artifact path is unsafe or disagrees with its media type.");
    } else if (!["policy_excluded", "unplaced_input"].includes(item.omitted_reason as string)) refuse("an unavailable artifact needs a supported omission reason.");
    const local = view.artifacts.find((artifact) => artifact.digest === item.digest);
    if (!local || local.kind !== item.media_type || local.name !== item.name || local.size !== item.size || local.available !== (item.path !== null)) refuse("the inspection view and artifact inventory disagree.");
  }
  if (view.artifacts.length !== seen.size) refuse("the inspection view and artifact inventory disagree.");
  if (!value.disclosure.diagnostics && (view.warnings.length || view.nodes.some((node) => node.error !== null) || layout.diagnostics.length)) refuse("withheld diagnostics were unexpectedly disclosed.");
  for (const values of [view.inputs, view.outputs, ...view.nodes.flatMap((node) => [node.with, node.outputs])]) for (const item of Object.values(values)) validateValue(item, value.disclosure.values, seen);
  validateReferences(view, layout, value.artifacts as ExportArtifact[]);
  return value as unknown as RunExport;
}

/** Resolve confined export paths under an explicitly selected HTTP document location. */
export function exportArtifactUrl(loaded: LoadedRunExport, id: string): string | null {
  const artifact = loaded.manifest.artifacts.find((item) => item.digest === id);
  if (!artifact?.path) return null;
  if (!digest(artifact.digest) || !mediaExtensions[artifact.media_type] || artifact.path !== `artifacts/${artifact.digest}.${mediaExtensions[artifact.media_type]}`) refuse("the resource path is unsafe.");
  const base = new URL(".", loaded.manifest_url);
  const resolved = new URL(artifact.path, base);
  if (!["https:", "http:"].includes(base.protocol) || base.username || base.password || base.search || base.hash
    || resolved.origin !== base.origin || !resolved.pathname.startsWith(base.pathname)) refuse("the resource base is unsafe.");
  return resolved.href;
}

/** Integrity is checked on the exact downloaded UTF-8 bytes before JSON parsing. */
export async function readRunExport(pin: PinnedRunExport, signal?: AbortSignal, fetcher: typeof fetch = fetch): Promise<LoadedRunExport> {
  if (!digest(pin.sha256)) refuse("the manifest pin is malformed.");
  const url = new URL(pin.url, typeof location === "undefined" ? undefined : location.href);
  if (!["https:", "http:"].includes(url.protocol) || url.username || url.password || url.search || url.hash) refuse("the manifest URL is unsafe.");
  const response = await fetcher(url.href, { signal, cache: "force-cache", redirect: "error" });
  if (!response.ok) throw new Error(`The recorded workflow could not be loaded (HTTP ${response.status}).`);
  const bytes = await response.arrayBuffer();
  if (bytes.byteLength > 32 * 1024 * 1024) refuse("the manifest is too large.");
  const hash = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), (part) => part.toString(16).padStart(2, "0")).join("");
  if (hash !== pin.sha256) throw new Error("The recorded workflow failed its integrity check. The downloaded manifest differs from the published content.");
  let value: unknown;
  try { value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)); }
  catch { throw new Error("The recorded workflow manifest is not valid UTF-8 JSON."); }
  return { manifest: parseRunExport(value), manifest_url: url.href };
}

/** Share wiring, scope projection and geometry while resolving previews through the static boundary. */
export function exportCanvasGraph(loaded: LoadedRunExport, scope: string | null = null): CanvasGraph {
  const { view, artifacts } = loaded.manifest;
  const flat = flatCanvasGraph(view);
  const inventory = new Map(artifacts.map((item) => [item.digest, item]));
  for (const node of flat.nodes) {
    const source = view.nodes.find((item) => item.id === node.source_id);
    if (!source) continue;
    const preview = cardPreview(Object.entries(source.outputs).flatMap(([port, value]) => exportFileDigests(value).flatMap((id) => {
      const artifact = inventory.get(id);
      const url = artifact ? exportArtifactUrl(loaded, id) : null;
      return artifact && url ? [{ digest: id, media_type: artifact.media_type, url, label: `${port}: ${artifact.name}` }] : [];
    })));
    if (preview) node.preview = preview;
  }
  return projectScopes(view, flat, scope);
}
