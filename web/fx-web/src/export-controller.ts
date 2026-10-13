import { exportCanvasGraph, readRunExport, type LoadedRunExport, type PinnedRunExport } from "./export";
import type { CanvasGraph, LayoutReport } from "./graph";
import { WorkflowNavigation, type Breadcrumb } from "./navigation";
import { navigationScopes } from "./scopes";

export interface ExportViewerState {
  recording: LoadedRunExport | null;
  loading: boolean;
  error: string | null;
  graph: CanvasGraph;
  layout: LayoutReport | null;
  scope: string | null;
  selected: string | null;
  breadcrumbs: Breadcrumb[];
}

/** Static recording state: one verified read, shared navigation, and no service or polling lifecycle. */
export class ExportViewerController {
  private state: ExportViewerState = { recording: null, loading: false, error: null, graph: { nodes: [], edges: [] }, layout: null, scope: null, selected: null, breadcrumbs: [] };
  private navigation = new WorkflowNavigation();
  private listeners = new Set<() => void>();
  private request: AbortController | null = null;

  constructor(private readonly reader: typeof readRunExport = readRunExport) {}

  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(next: Partial<ExportViewerState>) {
    this.state = { ...this.state, ...next };
    for (const listener of this.listeners) listener();
  }

  async load(pin: PinnedRunExport) {
    this.request?.abort();
    const request = new AbortController();
    this.request = request;
    this.navigation = new WorkflowNavigation();
    this.update({ recording: null, loading: true, error: null, graph: { nodes: [], edges: [] }, layout: null, selected: null, scope: null, breadcrumbs: [] });
    try {
      const recording = await this.reader(pin, request.signal);
      if (request.signal.aborted || this.request !== request) return;
      this.navigation.setScopes(navigationScopes(recording.manifest.view.scopes), recording.manifest.view.workflow.title);
      this.update({ recording, loading: false, layout: recording.manifest.layout, ...this.project(recording) });
    } catch (error) {
      if (request.signal.aborted || this.request !== request) return;
      this.update({ loading: false, error: error instanceof Error ? error.message : "The recorded workflow could not be loaded." });
    }
  }

  private project(recording: LoadedRunExport) {
    const state = this.navigation.getSnapshot();
    const graph = exportCanvasGraph(recording, state.scope);
    const selected = graph.nodes.some((node) => node.id === state.selected) ? state.selected : null;
    this.navigation.select(selected);
    return { graph, selected, scope: state.scope, breadcrumbs: state.breadcrumbs };
  }

  select = (id: string | null) => {
    if (id !== null && !this.state.graph.nodes.some((node) => node.id === id)) return false;
    this.navigation.select(id);
    this.update({ selected: id });
    return true;
  };
  openScope = (id: string) => {
    if (!this.state.recording || !this.state.graph.nodes.some((node) => node.kind === "workflow" && node.scope_id === id) || !this.navigation.open(id)) return false;
    this.update(this.project(this.state.recording));
    return true;
  };
  goToScope = (id: string | null) => {
    if (!this.state.recording || !this.navigation.goTo(id)) return false;
    this.update(this.project(this.state.recording));
    return true;
  };
  back = () => {
    const breadcrumbs = this.navigation.getSnapshot().breadcrumbs;
    if (breadcrumbs.length > 1) this.goToScope(breadcrumbs[breadcrumbs.length - 2].id);
  };

  /** Restore a deep link by walking recorded parent scopes, never by guessing path segments. */
  restore(scope: string | null, selected: string | null) {
    if (!this.state.recording) return false;
    const scopes = new Map(navigationScopes(this.state.recording.manifest.view.scopes).map((item) => [item.id, item]));
    if (scope !== null && !scopes.has(scope)) return false;
    this.navigation.goTo(null);
    const ancestors: string[] = [];
    for (let at = scope; at !== null; at = scopes.get(at)?.parent ?? null) ancestors.unshift(at);
    for (const id of ancestors) if (!this.navigation.open(id)) return false;
    this.update(this.project(this.state.recording));
    return this.select(selected);
  }

  dispose() { this.request?.abort(); this.request = null; this.listeners.clear(); }
}
