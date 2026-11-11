import { ViewerProtocol } from "../../../js/fx/src/viewer-protocol.js";
import type { CanvasGraph, LayoutReport } from "./graph";
import type { ViewerView } from "./index";
import { navigationScopes } from "./scopes";

export class ViewerPresentationError extends Error {
  constructor(readonly code: string, message: string) { super(message); }
}

export interface ViewerPresentationState {
  view: ViewerView | null;
  graph: CanvasGraph;
  layout: LayoutReport | null;
  scope: string | null;
  selected: string | null;
  error: string | null;
}

export interface ViewerPresentationSource {
  getSnapshot(): ViewerPresentationState;
  restore(scope: string | null, selected: string | null): boolean;
  /** Supply only an already verified reference; never derive one from a URL guess. */
  reference?(): ViewerProtocol.Reference | undefined;
}

/** Projects recorded evidence references and controls only the existing presentation. */
export class ViewerPresentation {
  private canvas: ((state: ViewerPresentationState, focus: string | "fit" | null) => void) | null = null;
  constructor(private readonly source: ViewerPresentationSource) {}

  attachCanvas(apply: (state: ViewerPresentationState, focus: string | "fit" | null) => void): () => void {
    this.canvas = apply;
    return () => { if (this.canvas === apply) this.canvas = null; };
  }

  getContext = (): ViewerProtocol.Context => {
    const state = this.source.getSnapshot();
    const view = state.view;
    if (!view) throw new ViewerPresentationError("not_ready", "No workflow evidence is loaded.");
    const subject: ViewerProtocol.Subject = {
      kind: view.kind === "fx-graph-v1" ? "plan" : "run",
      workflow: { id: view.workflow.id },
    };
    if (view.kind === "fx-graph-v1" && ViewerProtocol.isPortableSource(view.workflow.file)) subject.workflow!.source = view.workflow.file;
    const reference = this.source.reference?.();
    if (reference && ViewerProtocol.isReference(reference)) subject.reference = reference;
    const card = state.graph.nodes.find((node) => node.id === state.selected);
    let selection: ViewerProtocol.Selection | null = null;
    if (card?.kind === "workflow" && card.scope_id) selection = { kind: "scope", scope_id: card.scope_id };
    else if (card?.kind === "boundary" && card.scope_id && card.side) selection = { kind: "scope_boundary", scope_id: card.scope_id, side: card.side };
    else if (card?.pending) selection = { kind: "pending_repeat", path: card.source_id };
    else if (card) {
      const records = view.kind === "fx-graph-v1" ? view.instances : view.nodes;
      const record = records.find((node) => node.id === card.source_id);
      if (record) {
        selection = { kind: "instance", instance_id: record.id, path: record.path };
        if (record.step !== undefined) selection.step = record.step;
        if (record.take?.length) selection.take = [...record.take];
      }
    }
    const context: ViewerProtocol.Context = { subject, scope_id: state.scope, selection, sampled_at: Date.now() };
    if (state.error) context.retained = true;
    if (!ViewerProtocol.isContext(context)) throw new ViewerPresentationError("unavailable", "The recorded context cannot be represented by this viewer protocol.");
    return context;
  };

  present = (action: ViewerProtocol.Action): ViewerProtocol.Context => {
    if (!ViewerProtocol.isAction(action)) throw new ViewerPresentationError("invalid_request", "Unknown or malformed presentation action.");
    this.getContext();
    if (!this.canvas) throw new ViewerPresentationError("not_ready", "The workflow canvas is not mounted.");
    const state = this.source.getSnapshot();
    const view = state.view!;
    let scope = state.scope;
    let selected = state.selected;
    let focus: string | "fit" | null = null;
    switch (action.action) {
      case "reveal_instance": {
        const records = view.kind === "fx-graph-v1" ? view.instances : view.nodes;
        if (!records.some((record) => record.id === action.instance_id)) throw new ViewerPresentationError("not_found", "The recorded instance does not exist in this view.");
        const scopes = new Map((view.scopes ?? []).map((item) => [item.id, item]));
        let owner = (view.scopes ?? []).find((item) => item.nodes.includes(action.instance_id));
        while (owner && owner.kind !== "workflow") owner = owner.parent === null ? undefined : scopes.get(owner.parent);
        scope = owner?.id ?? null;
        selected = `instance:${action.instance_id}`;
        focus = selected;
        break;
      }
      case "open_scope":
        scope = action.scope_id;
        if (scope !== null && !navigationScopes(view.scopes).some((item) => item.id === scope)) throw new ViewerPresentationError("not_found", "The recorded workflow scope does not exist in this view.");
        selected = null;
        focus = "fit";
        break;
      case "clear_selection": selected = null; break;
      case "fit": focus = "fit"; break;
    }
    if (!this.source.restore(scope, selected)) throw new ViewerPresentationError("not_found", "The presentation target is not available in this view.");
    this.canvas(this.source.getSnapshot(), focus);
    return this.getContext();
  };
}
