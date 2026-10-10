import { canvasGraph, readView, type Artifact, type CanvasGraph, type CanvasStep, type LayoutReport, type ViewerView } from "./index";
import { WorkflowNavigation, type Breadcrumb } from "./navigation";
import { navigationScopes, type InterfaceBinding } from "./scopes";
import { cancellationRequested, ObservationError, observationReader, readLayout, type RunObservationReader } from "./observation";
import { validateViewerApiBase } from "./route";

export interface ViewerState {
  view: ViewerView | null;
  graph: CanvasGraph;
  /** The engine's cells and member order; null until read, or for a reader that serves none. */
  layout: LayoutReport | null;
  selected: string | null;
  loading: boolean;
  error: string | null;
  updated: Date | null;
  scope: string | null;
  breadcrumbs: Breadcrumb[];
  scopeNode: CanvasStep | null;
  cancelling: boolean;
}

export interface ViewerPollingScheduler {
  schedule(callback: () => void, delay: number): unknown;
  cancel(handle: unknown): void;
}

export interface ViewerControllerOptions {
  /** Custom view readers remain static unless observation is explicitly supplied. */
  observation?: RunObservationReader | false;
  scheduler?: ViewerPollingScheduler;
  apiBase?: string;
  /** Reads a saved plan's layout report (a run's comes with its snapshot); defaults to the host's `layout` route with the default reader. */
  layout?: ((signal: AbortSignal) => Promise<LayoutReport>) | false;
}

const pollingScheduler: ViewerPollingScheduler = {
  schedule: (callback, delay) => setTimeout(callback, delay),
  cancel: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/** Owns reads, serial observation, navigation and selection without a presentation framework. */
export class ViewerController {
  private state: ViewerState = { view: null, graph: { nodes: [], edges: [] }, layout: null, selected: null, loading: true, error: null, updated: null, scope: null, breadcrumbs: [{ id: null, title: "Workflow" }], scopeNode: null, cancelling: false };
  private navigation = new WorkflowNavigation();
  private listeners = new Set<() => void>();
  private request: AbortController | null = null;
  private readonly reader: typeof readView;
  private readonly observation: RunObservationReader | null;
  private readonly layoutReader: ((signal: AbortSignal) => Promise<LayoutReport>) | null;
  private readonly scheduler: ViewerPollingScheduler;
  private readonly apiBase: string;
  private timer: unknown = null;
  private cursor: string | null = null;
  private observingRun = false;
  private active = false;
  private failures = 0;
  constructor(reader?: typeof readView, options: ViewerControllerOptions = {}) {
    this.apiBase = validateViewerApiBase(options.apiBase ?? "/api");
    const defaultReader = reader === undefined || reader === readView;
    this.reader = defaultReader ? (signal) => readView(signal, this.apiBase) : reader;
    this.observation = options.observation === false ? null : options.observation ?? (defaultReader ? observationReader(this.apiBase) : null);
    this.scheduler = options.scheduler ?? pollingScheduler;
    this.layoutReader = options.layout === false ? null : options.layout ?? (defaultReader ? (signal) => readLayout(signal, this.apiBase) : null);
  }
  /** A report that cannot be read keeps the previous one; the canvas still draws. */
  private async readLayout(signal: AbortSignal) {
    if (!this.layoutReader) return this.state.layout;
    try { return await this.layoutReader(signal); } catch { return this.state.layout; }
  }

  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private update(next: Partial<ViewerState>) {
    this.state = { ...this.state, ...next };
    this.listeners.forEach((listener) => listener());
  }
  select = (id: string | null) => {
    if (id !== null && !this.state.graph.nodes.some((node) => node.id === id)) return;
    this.navigation.select(id);
    this.update({ selected: id });
  };
  referenceTarget = (source: string, kind?: InterfaceBinding["source_kind"]): string | null => {
    const visible = new Set(this.state.graph.nodes.map((node) => node.id));
    const direct = kind === "scope_input" ? `boundary:input:${source}`
      : kind === "scope_output" ? source : `instance:${source}`;
    if (visible.has(direct)) return direct;
    if (kind === "scope_input" || kind === "scope_output") return null;
    const scopes = this.state.view?.scopes ?? [];
    const byId = new Map(scopes.map((scope) => [scope.id, scope]));
    let scope = scopes.find((scope) => scope.nodes.includes(source));
    const visited = new Set<string>();
    while (scope && !visited.has(scope.id)) {
      visited.add(scope.id);
      if (scope.kind === "workflow" && visible.has(scope.id)) return scope.id;
      scope = scope.parent === null ? undefined : byId.get(scope.parent);
    }
    return null;
  };
  selectReference = (source: string, kind?: InterfaceBinding["source_kind"]) => {
    const target = this.referenceTarget(source, kind);
    if (target !== null) this.select(target);
  };
  private project(view: ViewerView) {
    const navigation = this.navigation.getSnapshot();
    const graph = canvasGraph(view, navigation.scope, this.apiBase);
    const selected = graph.nodes.some((node) => node.id === navigation.selected) ? navigation.selected : null;
    this.navigation.select(selected);
    const parentScope = navigation.breadcrumbs.at(-2)?.id ?? null;
    const scopeNode = navigation.scope === null ? null : canvasGraph(view, parentScope, this.apiBase).nodes.find((node) => node.scope_id === navigation.scope) ?? null;
    return { graph, selected, scope: navigation.scope, breadcrumbs: navigation.breadcrumbs, scopeNode };
  }
  openScope = (id: string) => {
    if (!this.state.view || !this.state.graph.nodes.some((node) => node.kind === "workflow" && node.scope_id === id)) return;
    if (this.navigation.open(id)) this.update(this.project(this.state.view));
  };
  goToScope = (id: string | null) => {
    if (this.state.view && this.navigation.goTo(id)) this.update(this.project(this.state.view));
  };
  back = () => {
    const breadcrumbs = this.navigation.getSnapshot().breadcrumbs;
    if (breadcrumbs.length > 1) this.goToScope(breadcrumbs[breadcrumbs.length - 2].id);
  };
  private apply(view: ViewerView, layout: LayoutReport | null, cancelling = false) {
    this.navigation.setScopes(navigationScopes(view.scopes), view.workflow.title);
    this.update({ view, layout, ...this.project(view), cancelling, error: null, updated: new Date() });
  }
  private cancelTimer() {
    if (this.timer !== null) this.scheduler.cancel(this.timer);
    this.timer = null;
  }
  private schedulePoll() {
    this.cancelTimer();
    if (!this.active || !this.observingRun || !this.observation) return;
    // A slow request never overlaps the next. Transient failures back off to at most 8 s.
    const delay = 1000 * 2 ** Math.min(this.failures, 3);
    this.timer = this.scheduler.schedule(() => { this.timer = null; void this.poll(); }, delay);
  }
  private async snapshot(request: AbortController) {
    const snapshot = await this.observation!.snapshot(request.signal);
    if (request.signal.aborted) return;
    this.cursor = snapshot.cursor;
    this.apply(snapshot.view, snapshot.layout ?? this.state.layout, cancellationRequested(snapshot.events));
  }
  private async poll() {
    if (!this.active || !this.observingRun || !this.observation) return;
    const request = new AbortController();
    this.request = request;
    try {
      try {
        if (this.cursor === null) { await this.snapshot(request); this.failures = 0; return; }
        const batch = await this.observation.events(this.cursor, request.signal);
        if (request.signal.aborted) return;
        if (batch.events.length > 0) await this.snapshot(request);
        else {
          this.cursor = batch.cursor;
          if (this.state.error !== null) this.update({ error: null });
        }
      } catch (error) {
        if (!request.signal.aborted && error instanceof ObservationError && error.requiresSnapshot) await this.snapshot(request);
        else throw error;
      }
      this.failures = 0;
    } catch (error) {
      if (!request.signal.aborted) {
        this.failures++;
        this.update({ error: error instanceof Error ? error.message : "The run could not be observed." });
      }
    } finally {
      if (!request.signal.aborted) this.schedulePoll();
    }
  }
  refresh = async () => {
    this.active = true;
    this.cancelTimer();
    this.request?.abort();
    const request = new AbortController();
    this.request = request;
    this.update({ loading: true });
    try {
      const view = await this.reader(request.signal);
      if (request.signal.aborted) return;
      this.observingRun = view.kind === "fx-viewer-run-v1" && this.observation !== null;
      if (this.observingRun) await this.snapshot(request);
      else {
        const layout = await this.readLayout(request.signal);
        if (request.signal.aborted) return;
        this.cursor = null;
        this.apply(view, layout);
      }
      this.failures = 0;
    } catch (error) {
      if (!request.signal.aborted) { this.failures++; this.update({ error: error instanceof Error ? error.message : "The workflow could not be loaded." }); }
    } finally {
      if (!request.signal.aborted) { this.update({ loading: false }); this.schedulePoll(); }
    }
  };
  dispose() { this.active = false; this.cancelTimer(); this.request?.abort(); }
}

export interface ViewportState { x: number; y: number; zoom: number }
export interface CanvasWheel { deltaX: number; deltaY: number; deltaMode: number; ctrlKey: boolean }

/** Screen-space camera only; it never changes graph positions or workflow data. */
export class CanvasViewport {
  private state: ViewportState = { x: 0, y: 0, zoom: 1 };
  private minimumZoom = 0.01;
  getSnapshot() { return { ...this.state }; }
  restore(snapshot: ViewportState) {
    if (![snapshot.x, snapshot.y, snapshot.zoom].every(Number.isFinite) || snapshot.zoom <= 0 || snapshot.zoom > 3) return;
    this.minimumZoom = Math.min(0.01, snapshot.zoom / 10);
    this.state = { ...snapshot };
  }
  pan(dx: number, dy: number) {
    this.state = { ...this.state, x: this.state.x + dx, y: this.state.y + dy };
  }
  wheel(event: CanvasWheel, point: { x: number; y: number }, size: { width: number; height: number }) {
    // Trackpads report pixels; mouse wheels may report lines or pages instead.
    const mode = event.deltaMode;
    const dx = event.deltaX * (mode === 1 ? 16 : mode === 2 ? size.width : 1);
    const dy = event.deltaY * (mode === 1 ? 16 : mode === 2 ? size.height : 1);
    if (event.ctrlKey) this.zoomAt(Math.exp(-dy * 0.01), point);
    else this.pan(-dx, -dy);
  }
  zoomAt(factor: number, point: { x: number; y: number }) {
    const zoom = Math.max(this.minimumZoom, Math.min(3, this.state.zoom * factor));
    const ratio = zoom / this.state.zoom;
    this.state = { x: point.x - (point.x - this.state.x) * ratio, y: point.y - (point.y - this.state.y) * ratio, zoom };
  }
  fit(bounds: { x?: number; y?: number; width: number; height: number }, size: { width: number; height: number }) {
    if (size.width <= 0 || size.height <= 0) return;
    const zoom = Math.min(1.1, Math.max(1, size.width - 64) / Math.max(1, bounds.width), Math.max(1, size.height - 64) / Math.max(1, bounds.height));
    this.minimumZoom = Math.min(0.01, zoom / 10);
    this.state = { x: (size.width - bounds.width * zoom) / 2 - (bounds.x ?? 0) * zoom, y: (size.height - bounds.height * zoom) / 2 - (bounds.y ?? 0) * zoom, zoom };
  }
}

export interface ArtifactState { text: string | null; loading: boolean; error: boolean }
const textKinds = new Set(["json", "application/json", "text", "text/plain", "annotations"]);
export function canPreviewText(artifact: Artifact) { return textKinds.has(artifact.kind) && artifact.size <= 1024 * 1024; }

/** Artifact request and retry state; a new record from Refresh resets a failed preview. */
export class ArtifactController {
  private state: ArtifactState = { text: null, loading: false, error: false };
  private listeners = new Set<() => void>();
  private request: AbortController | null = null;
  constructor(private readonly reader: (url: string, signal: AbortSignal) => Promise<string> = async (url, signal) => {
    const response = await fetch(url, { signal });
    if (!response.ok) throw new Error("Artifact preview unavailable");
    return response.text();
  }) {}
  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(next: ArtifactState) { this.state = next; this.listeners.forEach((listener) => listener()); }
  fail = () => { this.update({ ...this.state, loading: false, error: true }); };
  async load(artifact: Artifact) {
    this.request?.abort();
    const request = new AbortController();
    this.request = request;
    const fetchText = artifact.available && artifact.url !== null && canPreviewText(artifact);
    this.update({ text: null, loading: fetchText, error: false });
    if (!fetchText) return;
    try {
      const text = await this.reader(artifact.url!, request.signal);
      if (!request.signal.aborted) this.update({ text, loading: false, error: false });
    } catch {
      if (!request.signal.aborted) this.fail();
    }
  }
  dispose() { this.request?.abort(); }
}
