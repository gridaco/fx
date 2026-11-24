import { canvasGraph, readView, type Artifact, type CanvasGraph, type CanvasStep, type LayoutReport, type ViewerView } from "./index";
import { WorkflowNavigation, type Breadcrumb } from "./navigation";
import { navigationScopes } from "./scopes";
import { cancellationRequested, ObservationError, observationReader, readLayout, type ObservedInvocation, type RunObservationReader } from "./observation";
import { validateViewerApiBase } from "./route";
import { parseViewerConfig, readViewerConfig, temporaryDisplaySelection, viewerDisplay, type ViewerConfig, type ViewerDisplay } from "./display";

export interface ViewerState {
  view: ViewerView | null;
  graph: CanvasGraph;
  /** The engine's cells and member order; null until read, or for a reader that serves none. */
  layout: LayoutReport | null;
  selected: string | null;
  loading: boolean;
  error: string | null;
  scope: string | null;
  breadcrumbs: Breadcrumb[];
  scopeNode: CanvasStep | null;
  cancelling: boolean;
  runActive: boolean;
  /** The latest invocation represented by this canvas snapshot, even after it ends. */
  invocationId: string | null;
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
  /** Optional host overrides; normal native viewers read the project's configuration. */
  display?: Partial<ViewerDisplay>;
  /** Custom readers have no configuration request unless explicitly supplied. */
  viewerConfig?: ((signal: AbortSignal) => Promise<ViewerConfig>) | false;
}

const pollingScheduler: ViewerPollingScheduler = {
  schedule: (callback, delay) => setTimeout(callback, delay),
  cancel: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/** Owns reads, serial observation, navigation and selection without a presentation framework. */
export class ViewerController {
  private state: ViewerState = { view: null, graph: { nodes: [], edges: [] }, layout: null, selected: null, loading: true, error: null, scope: null, breadcrumbs: [{ id: null, title: "Workflow" }], scopeNode: null, cancelling: false, runActive: false, invocationId: null };
  private navigation = new WorkflowNavigation();
  private listeners = new Set<() => void>();
  private request: AbortController | null = null;
  private readonly reader: typeof readView;
  private readonly observation: RunObservationReader | null;
  private readonly layoutReader: ((signal: AbortSignal) => Promise<LayoutReport>) | null;
  private readonly scheduler: ViewerPollingScheduler;
  private readonly apiBase: string;
  private readonly configReader: ((signal: AbortSignal) => Promise<ViewerConfig>) | null;
  private readonly displayOverrides: Partial<ViewerDisplay> | undefined;
  private display = viewerDisplay();
  private timer: unknown = null;
  private cursor: string | null = null;
  private invocation?: ObservedInvocation;
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
    this.configReader = options.viewerConfig === false ? null : options.viewerConfig ?? (defaultReader ? (signal) => readViewerConfig(signal, this.apiBase) : null);
    this.display = viewerDisplay(options.display);
    this.displayOverrides = options.display === undefined ? undefined : { ...options.display };
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
    if (id !== null && !this.state.graph.nodes.some((node) => node.id === id) && !this.state.graph.notes?.some((note) => note.id === id)) return;
    const dismissSource = id !== this.state.selected && temporaryDisplaySelection(this.state.graph, this.state.selected, this.display);
    this.navigation.select(id);
    this.update(dismissSource && this.state.view ? this.project(this.state.view) : { selected: id });
  };
  private project(view: ViewerView) {
    const navigation = this.navigation.getSnapshot();
    const graph = canvasGraph(view, navigation.scope, this.apiBase, navigation.selected, this.display);
    const selected = (graph.nodes.some((node) => node.id === navigation.selected) || graph.notes?.some((note) => note.id === navigation.selected)) ? navigation.selected : null;
    this.navigation.select(selected);
    const parentScope = navigation.breadcrumbs.at(-2)?.id ?? null;
    const scopeNode = navigation.scope === null ? null : canvasGraph(view, parentScope, this.apiBase, null, this.display).nodes.find((node) => node.scope_id === navigation.scope) ?? null;
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
  /** Restore presentation using recorded scope membership, rejecting before mutation. */
  restore(scope: string | null, selected: string | null): boolean {
    const view = this.state.view;
    if (!view || (scope !== null && !navigationScopes(view.scopes).some((item) => item.id === scope))) return false;
    const graph = canvasGraph(view, scope, this.apiBase, selected, this.display);
    if (selected !== null && !graph.nodes.some((node) => node.id === selected) && !graph.notes?.some((note) => note.id === selected)) return false;
    if (!this.navigation.restore(scope, selected)) return false;
    this.update(this.project(view));
    return true;
  }
  private apply(view: ViewerView, layout: LayoutReport | null, cancelling = false, runActive = false, invocationId: string | null = null) {
    this.navigation.setScopes(navigationScopes(view.scopes), view.workflow.title);
    this.update({ view, layout, ...this.project(view), cancelling, runActive, invocationId, error: null });
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
    const invocation = snapshot.events.reduce<ObservedInvocation | undefined>((latest, event) => event.event === "run_started" ? event : latest, undefined);
    this.invocation = ["unfinished", "pausing"].includes(snapshot.view.state) ? invocation : undefined;
    this.apply(snapshot.view, snapshot.layout ?? this.state.layout, cancellationRequested(snapshot.events),
      this.invocation !== undefined && snapshot.run_active === true, invocation?.invocation_id ?? null);
  }
  private async poll() {
    if (!this.active || !this.observingRun || !this.observation) return;
    const request = new AbortController();
    this.request = request;
    try {
      try {
        if (this.cursor === null) { await this.snapshot(request); this.failures = 0; return; }
        const batch = await this.observation.events(this.cursor, request.signal, this.invocation);
        if (request.signal.aborted) return;
        if (batch.events.length > 0) await this.snapshot(request);
        else {
          this.cursor = batch.cursor;
          const runActive = this.invocation !== undefined && batch.run_active === true;
          if (this.state.error !== null || this.state.runActive !== runActive) this.update({ error: null, runActive });
        }
      } catch (error) {
        if (!request.signal.aborted && error instanceof ObservationError && error.requiresSnapshot) await this.snapshot(request);
        else throw error;
      }
      this.failures = 0;
    } catch (error) {
      if (!request.signal.aborted) {
        this.failures++;
        this.update({ error: error instanceof Error ? error.message : "The run could not be observed.", runActive: false });
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
    this.observingRun = false;
    this.cursor = null;
    this.invocation = undefined;
    this.update({ loading: true, runActive: false });
    try {
      const [view, config] = await Promise.all([
        this.reader(request.signal),
        this.configReader ? this.configReader(request.signal) : Promise.resolve({ display: viewerDisplay() }),
      ]);
      if (request.signal.aborted) return;
      this.display = viewerDisplay(this.displayOverrides, parseViewerConfig(config).display);
      this.observingRun = view.kind === "fx-viewer-run-v1" && this.observation !== null;
      if (this.observingRun) await this.snapshot(request);
      else {
        const layout = await this.readLayout(request.signal);
        if (request.signal.aborted) return;
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
/** Screen pixels along each edge of the canvas that floating panels cover. */
export interface CanvasInset { left: number; right: number; top: number; bottom: number }
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
  /**
   * Centres a box in the part of the canvas the inset leaves free. A zoom at which titles read
   * stays; below it the camera zooms in to 100 %, and never past what fits. Honor narrow usable
   * space; only fall back to the canvas when the inset leaves no room beyond the padding.
   */
  reveal(box: { x: number; y: number; width: number; height: number }, size: { width: number; height: number }, inset: CanvasInset, legible: number) {
    let area = { x: inset.left, y: inset.top, width: size.width - inset.left - inset.right, height: size.height - inset.top - inset.bottom };
    if (area.width <= 32 || area.height <= 32) area = { x: 0, y: 0, width: size.width, height: size.height };
    if (area.width <= 0 || area.height <= 0) return;
    const fits = Math.min((area.width - 32) / box.width, (area.height - 32) / box.height);
    const zoom = Math.max(this.minimumZoom, Math.min(this.state.zoom >= legible ? this.state.zoom : 1, fits));
    this.state = { x: area.x + area.width / 2 - (box.x + box.width / 2) * zoom, y: area.y + area.height / 2 - (box.y + box.height / 2) * zoom, zoom };
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
