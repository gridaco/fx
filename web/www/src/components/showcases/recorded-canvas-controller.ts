import { CanvasController, ExportViewerController, exportArtifactUrl, type CanvasGraph, type ExportViewerState, type LoadedRunExport, type PinnedRunExport } from "@grida/fx-web";

export interface Recording extends PinnedRunExport { id: string; label: string }
export interface CanvasFragment { graph: boolean; recording: string | null; scope: string | null; node: string | null }

export function parseCanvasFragment(hash: string): CanvasFragment {
  const [mode, query] = hash.replace(/^#/, "").split("?", 2);
  const params = new URLSearchParams(query);
  return { graph: mode === "graph", recording: params.get("recording"), scope: params.get("scope"), node: params.get("node") };
}
export function canvasFragment(recording: string, scope: string | null, node: string | null): string {
  const params = new URLSearchParams({ recording });
  if (scope !== null) params.set("scope", scope);
  if (node !== null) params.set("node", node);
  return `#graph?${params}`;
}

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
export const isWithheldValue = (value: unknown, includeValues: boolean) => !includeValues && record(value) && value.withheld === true;

/** Owns the website's static presentation; the FX controller owns recorded graph/navigation state. */
export class RecordedCanvasController {
  private readonly viewer = new ExportViewerController();
  private readonly win: Window;
  private readonly abort = new AbortController();
  private unsubscribe: () => void;
  private canvas: CanvasController | null = null;
  private graph: CanvasGraph | null = null;
  private recording: string | null = null;
  private mode: "overview" | "graph" = "overview";
  private restore: CanvasFragment | null = null;

  constructor(private readonly root: HTMLElement, private readonly recordings: readonly Recording[]) {
    this.win = root.ownerDocument.defaultView!;
    this.unsubscribe = this.viewer.subscribe(() => this.render());
    root.addEventListener("click", this.click, { signal: this.abort.signal });
    root.addEventListener("change", this.change, { signal: this.abort.signal });
    root.addEventListener("keydown", this.keydown, { capture: true, signal: this.abort.signal });
    this.win.addEventListener("hashchange", this.route, { signal: this.abort.signal });
    this.win.addEventListener("popstate", this.route, { signal: this.abort.signal });
    this.route();
  }

  private element<T extends HTMLElement = HTMLElement>(selector: string): T { return this.root.querySelector<T>(selector)!; }
  private setMode(mode: "overview" | "graph") {
    this.mode = mode;
    this.element("[data-overview]").hidden = mode !== "overview";
    this.element("[data-graph]").hidden = mode !== "graph";
    for (const tab of this.root.querySelectorAll<HTMLElement>("[data-mode]")) {
      const active = tab.dataset.mode === mode;
      tab.setAttribute("aria-selected", String(active));
      tab.tabIndex = active ? 0 : -1;
    }
  }
  private route = () => {
    const fragment = parseCanvasFragment(this.win.location.hash);
    this.setMode(fragment.graph ? "graph" : "overview");
    if (!fragment.graph) return;
    this.restore = fragment;
    const selected = fragment.recording ?? this.recordings[0]?.id;
    if (!selected || !this.recordings.some((item) => item.id === selected)) {
      this.showError(selected ? "The requested recording is not part of this showcase." : "This showcase does not have a published run recording yet.");
      return;
    }
    if (selected !== this.recording) this.load(selected);
    else this.applyRestore();
  };
  private load(id: string) {
    const recording = this.recordings.find((item) => item.id === id);
    if (!recording) return;
    this.recording = id;
    this.canvas?.dispose(); this.canvas = null; this.graph = null;
    const select = this.root.querySelector<HTMLSelectElement>("[data-recording]");
    if (select) select.value = id;
    void this.viewer.load(recording);
  }
  private showError(message: string) {
    const status = this.element("[data-recording-status]");
    status.hidden = false;
    status.setAttribute("role", "alert");
    status.textContent = message;
    this.element("[data-workspace]").hidden = true;
  }
  private applyRestore() {
    const fragment = this.restore;
    if (!fragment || !this.viewer.getSnapshot().recording) return;
    this.restore = null;
    if (!this.viewer.restore(fragment.scope, fragment.node)) this.showError("The requested workflow or step is absent from this recording.");
  }
  private writeFragment(push = false) {
    const state = this.viewer.getSnapshot();
    const hash = this.mode === "graph" && this.recording ? canvasFragment(this.recording, state.scope, state.selected) : "";
    const url = `${this.win.location.pathname}${this.win.location.search}${hash}`;
    if (push) this.win.history.pushState(null, "", url); else this.win.history.replaceState(null, "", url);
  }
  private click = (event: MouseEvent) => {
    if (!(event.target instanceof Element)) return;
    const tab = event.target.closest<HTMLElement>("[data-mode]");
    if (tab && this.root.contains(tab)) {
      this.setMode(tab.dataset.mode === "graph" ? "graph" : "overview");
      if (this.mode === "graph" && !this.recording) {
        if (this.recordings[0]) this.load(this.recordings[0].id);
        else this.showError("This showcase does not have a published run recording yet.");
      }
      if (this.mode === "graph" && this.viewer.getSnapshot().recording) this.render();
      this.writeFragment(true);
      return;
    }
    if (event.target.closest("[data-close-inspector]")) { this.viewer.select(null); this.writeFragment(); }
    const crumb = event.target.closest<HTMLElement>("[data-scope]");
    if (crumb) { this.viewer.goToScope(crumb.dataset.scope || null); this.writeFragment(); }
  };
  private change = (event: Event) => {
    if (event.target instanceof HTMLSelectElement && event.target.matches("[data-recording]")) {
      this.restore = null;
      this.load(event.target.value);
      this.writeFragment();
    }
  };
  private keydown = (event: KeyboardEvent) => {
    if (event.key === "Escape" && this.viewer.getSnapshot().selected !== null && !event.defaultPrevented) {
      event.preventDefault(); this.viewer.select(null); this.writeFragment();
    }
    if (!(event.target instanceof HTMLElement) || !event.target.matches("[data-mode]")) return;
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const mode = event.key === "Home" ? "overview" : event.key === "End" ? "graph" : event.target.dataset.mode === "overview" ? "graph" : "overview";
    this.element<HTMLButtonElement>(`[data-mode="${mode}"]`).click();
    this.element(`[data-mode="${mode}"]`).focus();
  };
  private render() {
    const state = this.viewer.getSnapshot();
    if (state.error) { this.showError(state.error); return; }
    const status = this.element("[data-recording-status]");
    status.hidden = !state.loading;
    status.setAttribute("role", "status");
    status.textContent = "Loading the verified run recording…";
    this.element("[data-workspace]").hidden = state.recording === null;
    if (!state.recording) return;
    if (this.restore) { this.applyRestore(); return; }
    if (!this.canvas) this.canvas = new CanvasController(this.element("[data-canvas]"), (id) => { this.viewer.select(id); this.writeFragment(); }, (id) => { this.viewer.openScope(id); this.writeFragment(); }, () => { this.viewer.back(); this.writeFragment(); });
    if (state.graph !== this.graph) { this.canvas.setGraph(state.graph, state.scope, state.layout); this.graph = state.graph; }
    this.canvas.setSelection(state.selected);
    this.renderBreadcrumbs(state);
    this.renderInspector(state);
    const { view, disclosure, artifacts } = state.recording.manifest;
    this.element("[data-recording-summary]").textContent = `${view.state} · ${view.nodes.length} steps${view.charged_usd !== null ? ` · $${view.charged_usd.toFixed(2)}` : ""} · Recorded`;
    const omitted = artifacts.filter((item) => item.path === null).length;
    const note = this.element("[data-recording-disclosure]");
    note.hidden = false;
    note.textContent = `Includes recorded inputs and intermediate outputs. Overview shows the selected result. ${disclosure.values ? "Recorded values included." : "Text values withheld by the author."} ${disclosure.diagnostics ? "Diagnostics included." : "Diagnostics withheld."}${omitted ? ` ${omitted} artifact${omitted === 1 ? " is" : "s are"} omitted from this export; unavailable files are labeled in step details.` : ""}`;
  }
  private node(tag: string, text?: string) {
    const node = this.root.ownerDocument.createElement(tag);
    if (text !== undefined) node.textContent = text;
    return node;
  }
  private renderBreadcrumbs(state: ExportViewerState) {
    const root = this.element("[data-breadcrumbs]");
    root.replaceChildren(); root.hidden = state.breadcrumbs.length < 2;
    for (const [index, crumb] of state.breadcrumbs.entries()) {
      if (index) root.append(this.node("span", "/"));
      const button = this.node("button", crumb.title); button.dataset.scope = crumb.id ?? ""; root.append(button);
    }
  }
  private renderInspector(state: ExportViewerState) {
    const selected = state.graph.nodes.find((node) => node.id === state.selected);
    this.element("[data-inspector]").hidden = !selected;
    const body = this.element("[data-inspector-body]"); body.replaceChildren();
    if (!selected || !state.recording) return;
    const loaded = state.recording;
    const source = loaded.manifest.view.nodes.find((node) => node.id === selected.source_id);
    body.append(this.node("h2", selected.title), this.node("p", `${selected.state}${source?.duration_ms !== null && source?.duration_ms !== undefined ? ` · ${(source.duration_ms / 1000).toFixed(1)} s` : ""}`));
    if (selected.kind) {
      if (selected.child_count !== undefined) body.append(this.node("p", `${selected.child_count} recorded steps`));
      for (const error of selected.errors ?? []) body.append(this.node("p", `${error.title}: ${error.message}`));
      return;
    }
    if (!source) return;
    if (source.uses) body.append(this.node("p", source.uses));
    if (source.error) body.append(this.node("p", source.error));
    for (const [title, values] of [["Outputs", source.outputs], ["Inputs and settings", source.with]] as const) {
      if (!Object.keys(values).length) continue;
      body.append(this.node("h3", title));
      const list = this.node("dl");
      for (const [name, value] of Object.entries(values)) { const item = this.node("dd"); this.renderValue(item, value, loaded); list.append(this.node("dt", name), item); }
      body.append(list);
    }
  }
  private renderValue(target: HTMLElement, value: unknown, loaded: LoadedRunExport) {
    if (isWithheldValue(value, loaded.manifest.disclosure.values)) { target.append(this.node("p", "Withheld by the author")); return; }
    if (record(value) && record(value.file) && typeof value.file.digest === "string") {
      const digest = value.file.digest;
      const artifact = loaded.manifest.artifacts.find((item) => item.digest === digest);
      const url = exportArtifactUrl(loaded, digest);
      if (!artifact || !url) { target.append(this.node("p", `${artifact?.name ?? "Artifact"} · ${artifact?.omitted_reason === "unplaced_input" ? "input bytes were not stored in this run" : "omitted from this export"}`)); return; }
      if (artifact.media_type.startsWith("image/")) {
        const image = this.node("img") as HTMLImageElement; image.src = url; image.alt = artifact.name; image.loading = "lazy";
        image.addEventListener("error", () => { image.remove(); target.prepend(this.node("p", `${artifact.name} could not be loaded from the published recording.`)); }, { once: true, signal: this.abort.signal });
        target.append(image);
      }
      const link = this.node("a", `Open ${artifact.name}`) as HTMLAnchorElement; link.href = url; link.target = "_blank"; link.rel = "noopener noreferrer"; target.append(link);
      return;
    }
    if (record(value) && Array.isArray(value.list)) { for (const item of value.list) this.renderValue(target, item, loaded); return; }
    if (record(value) && Array.isArray(value.collection)) {
      for (const entry of value.collection) if (Array.isArray(entry) && entry.length === 2) { target.append(this.node("p", String(entry[0]))); this.renderValue(target, entry[1], loaded); }
      return;
    }
    target.append(this.node("pre", typeof value === "string" ? value : JSON.stringify(value, null, 2)));
  }
  dispose() { this.abort.abort(); this.unsubscribe(); this.viewer.dispose(); this.canvas?.dispose(); this.canvas = null; }
}
