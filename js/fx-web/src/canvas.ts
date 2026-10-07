import dagre from "@dagrejs/dagre";
import type { CanvasConnection, CanvasFrame, CanvasGraph, CanvasPort, CanvasStep } from "./graph";
import { CanvasViewport } from "./controller";
import { displayPortName } from "./ports";

export interface PositionedNode extends CanvasStep { x: number; y: number; width: number; height: number }
export interface PositionedEdge extends CanvasConnection { points: { x: number; y: number }[] }
export interface PositionedFrame extends CanvasFrame { x: number; y: number; width: number; height: number }
export interface CanvasLayout { nodes: PositionedNode[]; edges: PositionedEdge[]; frames?: PositionedFrame[]; width: number; height: number }

const PORT_TOP = 93;
const PORT_ROW = 34;

/** Geometry depends on declarations, never on image loading or measured DOM text. */
export function canvasNodeGeometry(node: CanvasStep) {
  if (!node.ports) return { width: 250, height: node.preview ? 246 : 106, preview_y: 61, settings_y: 0 };
  const rows = Math.max(node.ports.inputs.length, node.ports.outputs.length);
  const settings_y = 83 + Math.max(1, rows) * PORT_ROW;
  const preview_y = settings_y + (node.ports.settings.length ? 29 : 5);
  return { width: 360, height: preview_y + (node.preview ? 140 + 16 : 0) + (node.kind === "workflow" ? 70 : 42), preview_y, settings_y };
}
export function canvasNodeSize(node: CanvasStep) {
  const { width, height } = canvasNodeGeometry(node);
  return { width, height };
}

/** A named socket's position; unknown names have no invented attachment point. */
export function canvasPortAnchor(node: PositionedNode, direction: "input" | "output", name: string, kind?: CanvasPort["kind"]) {
  const ports = direction === "input" ? node.ports?.inputs : node.ports?.outputs;
  const index = ports?.findIndex((port) => port.name === name && (!kind || port.kind === kind)) ?? -1;
  if (index < 0) return undefined;
  return { x: node.x + (direction === "input" ? 0 : node.width), y: node.y + PORT_TOP + index * PORT_ROW };
}

export function layoutGraph(graph: CanvasGraph): CanvasLayout {
  // Dagre places node pairs; socket routing below preserves every original connection.
  // Its multigraph intersection pass fails on valid fan-out/join port topologies.
  const layout = new dagre.graphlib.Graph({ compound: Boolean(graph.frames?.length) }).setGraph({ rankdir: "LR", nodesep: 42, ranksep: 100, marginx: 20, marginy: 20 }).setDefaultEdgeLabel(() => ({}));
  const parallel = new Map<string, string[]>();
  for (const frame of graph.frames ?? []) layout.setNode(frame.id, {});
  for (const node of graph.nodes) layout.setNode(node.id, canvasNodeSize(node));
  for (const frame of graph.frames ?? []) {
    if (frame.parent) layout.setParent(frame.id, frame.parent);
    for (const id of frame.nodes) layout.setParent(id, frame.id);
  }
  for (const edge of graph.edges) {
    layout.setEdge(edge.source, edge.target);
    const pair = JSON.stringify([edge.source, edge.target]);
    const ids = parallel.get(pair) ?? [];
    ids.push(edge.id);
    parallel.set(pair, ids);
  }
  dagre.layout(layout);
  const nodes: PositionedNode[] = graph.nodes.map((node) => {
    const placed = layout.node(node.id);
    return { ...node, x: placed.x - placed.width / 2, y: placed.y - placed.height / 2, width: placed.width, height: placed.height };
  });
  const byId = new Map(nodes.map((node) => [node.id, node]));
  return {
    nodes,
    ...(graph.frames?.length ? { frames: graph.frames.map((frame) => {
      const placed = layout.node(frame.id);
      return { ...frame, x: placed.x - placed.width / 2, y: placed.y - placed.height / 2, width: placed.width, height: placed.height };
    }) } : {}),
    edges: graph.edges.map((edge) => {
      const points: { x: number; y: number }[] = layout.edge(edge.source, edge.target).points ?? [];
      const siblings = parallel.get(JSON.stringify([edge.source, edge.target]))!;
      const lane = ((siblings.indexOf(edge.id) + 1) / (siblings.length + 1) - 0.5) * 24;
      const source = byId.get(edge.source)!;
      const target = byId.get(edge.target)!;
      const from = edge.kind === "data" && edge.source_port !== undefined
        ? canvasPortAnchor(source, "output", edge.source_port, edge.source_kind === "fact" ? "fact" : undefined)
        : { x: source.x + source.width, y: source.y + 27 };
      const to = edge.kind === "data" && edge.target_port
        ? canvasPortAnchor(target, "input", edge.target_port)
        : { x: target.x, y: target.y + 27 };
      if (!from || !to) throw new Error(`Connection ${edge.id} refers to a missing named port.`);
      const corridor = points.slice(1, -1).map((point) => ({ x: point.x, y: point.y + lane }));
      return { ...edge, points: [from, { x: from.x + 16, y: from.y }, ...corridor, { x: to.x - 16, y: to.y }, to] };
    }),
    width: layout.graph().width ?? 0, height: layout.graph().height ?? 0,
  };
}

/** Nearby sockets use horizontal tangents; longer routes retain Dagre's detours. */
export function canvasEdgePath(points: PositionedEdge["points"]): string {
  const route = points.filter((point, index) => !index || point.x !== points[index - 1].x || point.y !== points[index - 1].y);
  if (!route.length) return "";
  const from = route[0], to = route.at(-1)!;
  const start = `M ${from.x} ${from.y}`;
  // One interior Dagre waypoint connects adjacent ranks. Its center-based height
  // need not lie between the actual sockets, so do not carry it into the curve.
  if (points.length <= 5 && to.x > from.x) {
    const reach = (to.x - from.x) / 2;
    return `${start} C ${from.x + reach} ${from.y} ${to.x - reach} ${to.y} ${to.x} ${to.y}`;
  }
  let path = start;
  for (let index = 1; index < route.length - 1; index++) {
    const before = route[index - 1], corner = route[index], after = route[index + 1];
    const incoming = Math.hypot(corner.x - before.x, corner.y - before.y);
    const outgoing = Math.hypot(after.x - corner.x, after.y - corner.y);
    const radius = Math.min(24, incoming / 2, outgoing / 2);
    const entry = { x: corner.x + (before.x - corner.x) * radius / incoming, y: corner.y + (before.y - corner.y) * radius / incoming };
    const exit = { x: corner.x + (after.x - corner.x) * radius / outgoing, y: corner.y + (after.y - corner.y) * radius / outgoing };
    path += ` L ${entry.x} ${entry.y} Q ${corner.x} ${corner.y} ${exit.x} ${exit.y}`;
  }
  return route.length > 1 ? `${path} L ${to.x} ${to.y}` : path;
}

const SVG = "http://www.w3.org/2000/svg";
function element<K extends keyof SVGElementTagNameMap>(tag: K, attributes: Record<string, string | number> = {}, text?: string): SVGElementTagNameMap[K] {
  const node = document.createElementNS(SVG, tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, String(value));
  if (text !== undefined) node.textContent = text;
  return node;
}
function short(text: string, limit: number) { return [...text].length > limit ? [...text].slice(0, limit - 1).join("") + "…" : text; }

/** Imperative, read-only SVG surface. DOM listeners, selection and camera live here. */
export class CanvasController {
  private static nextId = 0;
  private readonly id = `fx-canvas-${++CanvasController.nextId}`;
  private readonly svg = element("svg", { role: "group", "aria-label": "Workflow dependency canvas" });
  private readonly content = element("g");
  private readonly viewport = new CanvasViewport();
  private readonly nodes = new Map<string, SVGGElement>();
  private readonly edges = new Map<string, { group: SVGGElement; source: string; target: string }>();
  private readonly incidentEdges = new Map<string, Set<string>>();
  private highlightedEdges = new Set<string>();
  private viewportFrame: number | null = null;
  private readonly abort = new AbortController();
  private graphAbort = new AbortController();
  private readonly resize: ResizeObserver;
  private readonly toolbar = document.createElement("div");
  private readonly zoomLabel = document.createElement("span");
  private readonly empty = document.createElement("p");
  private layout: CanvasLayout = { nodes: [], edges: [], width: 0, height: 0 };
  private selected: string | null = null;
  private drag: { pointer: number; x: number; y: number; startX: number; startY: number; moved: boolean; background: boolean } | null = null;
  private didFit = false;
  private scope: string | null = null;
  private readonly cameras = new Map<string | null, { viewport: ReturnType<CanvasViewport["getSnapshot"]>; didFit: boolean }>();

  constructor(private readonly container: HTMLElement, private readonly onSelect: (id: string | null) => void,
    private readonly onOpenScope: (id: string) => void = () => {}, private readonly onBack: () => void = () => {}) {
    container.classList.add("fx-canvas");
    container.tabIndex = -1;
    const defs = element("defs");
    const marker = element("marker", { id: `${this.id}-arrow`, viewBox: "0 0 10 10", refX: 9, refY: 5, markerWidth: 6, markerHeight: 6, orient: "auto-start-reverse" });
    marker.append(element("path", { d: "M 0 0 L 10 5 L 0 10 z", fill: "#a1a1aa" }));
    const checker = element("pattern", { id: `${this.id}-checker`, patternUnits: "userSpaceOnUse", width: 16, height: 16 });
    checker.append(element("rect", { width: 16, height: 16, fill: "#fafafa" }));
    checker.append(element("path", { d: "M0 0h8v8H0z M8 8h8v8H8z", fill: "#ededf0" }));
    defs.append(marker, checker);
    this.svg.append(defs, this.content);
    this.toolbar.className = "fx-canvas-toolbar";
    const button = (label: string, text: string, action: () => void) => {
      const control = document.createElement("button");
      control.type = "button";
      control.title = label;
      control.setAttribute("aria-label", label);
      control.textContent = text;
      control.addEventListener("click", action, { signal: this.abort.signal });
      this.toolbar.append(control);
    };
    button("Zoom out", "−", () => this.zoom(1 / 1.25));
    this.zoomLabel.setAttribute("aria-live", "off");
    this.toolbar.append(this.zoomLabel);
    button("Zoom in", "+", () => this.zoom(1.25));
    button("Fit workflow", "Fit", () => this.fit());
    this.empty.className = "fx-canvas-empty";
    this.empty.textContent = "No expanded steps in this plan.";
    container.replaceChildren(this.svg, this.toolbar, this.empty);
    container.addEventListener("wheel", this.onWheel, { passive: false, signal: this.abort.signal });
    this.svg.addEventListener("pointerdown", this.onPointerDown, { signal: this.abort.signal });
    this.svg.addEventListener("pointermove", this.onPointerMove, { signal: this.abort.signal });
    this.svg.addEventListener("pointerup", this.onPointerUp, { signal: this.abort.signal });
    this.svg.addEventListener("pointercancel", this.onPointerUp, { signal: this.abort.signal });
    container.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.scope !== null && !event.defaultPrevented) { event.preventDefault(); this.onBack(); }
    }, { signal: this.abort.signal });
    this.resize = new ResizeObserver(() => { if (!this.didFit && this.layout.nodes.length) this.fit(); });
    this.resize.observe(container);
    this.applyViewport();
  }
  setGraph(graph: CanvasGraph, scope: string | null = null) {
    const navigating = scope !== this.scope;
    const restoringFocus = this.content.contains(document.activeElement);
    this.cancelDrag();
    if (navigating) {
      this.cameras.set(this.scope, { viewport: this.viewport.getSnapshot(), didFit: this.didFit });
      this.scope = scope;
      const camera = this.cameras.get(scope);
      this.didFit = camera?.didFit ?? false;
      if (camera) this.viewport.restore(camera.viewport);
    }
    this.graphAbort.abort();
    this.graphAbort = new AbortController();
    this.layout = layoutGraph(graph);
    const fragment = document.createDocumentFragment();
    this.nodes.clear();
    this.edges.clear();
    this.incidentEdges.clear();
    this.highlightedEdges.clear();
    const frames = this.layout.frames ?? [];
    const frameById = new Map(frames.map((frame) => [frame.id, frame]));
    const depth = (frame: PositionedFrame) => {
      let count = 0, parent = frame.parent;
      while (parent && frameById.has(parent) && count < frames.length) { count++; parent = frameById.get(parent)!.parent; }
      return count;
    };
    for (const frame of [...frames].sort((a, b) => depth(a) - depth(b))) {
      const group = element("g", { class: "fx-canvas-frame", "data-frame-id": frame.id, "aria-label": frame.title });
      group.append(element("rect", { x: frame.x, y: frame.y, width: frame.width, height: frame.height, rx: 12 }));
      group.append(element("text", { x: frame.x + 14, y: frame.y + 16 }, short(frame.title, 64)));
      fragment.append(group);
    }
    for (const edge of this.layout.edges) {
      const description = edge.kind === "data"
        ? `${displayPortName(edge.source_port!)} → ${edge.target_port}${edge.source_kind === "fact" ? " (fact)" : ""}`
        : `${edge.kind === "control" ? "Control" : "Dependency"}: ${edge.kinds.join(", ")}`;
      const d = canvasEdgePath(edge.points);
      const group = element("g", { class: "fx-canvas-connection", "data-connection-id": edge.id, "data-source-node": edge.source, "data-target-node": edge.target });
      const path = element("path", { d, class: `fx-canvas-edge fx-canvas-edge-${edge.kind ?? "dependency"}`, "data-edge-id": edge.id, "data-source-port": edge.source_port ?? "", "data-target-port": edge.target_port ?? "", "marker-end": `url(#${this.id}-arrow)`, role: "img", "aria-label": description });
      path.append(element("title", {}, description));
      const hit = element("path", { d, class: "fx-canvas-edge-hit", "aria-hidden": "true" });
      hit.append(element("title", {}, description));
      group.append(path, hit);
      group.addEventListener("pointerenter", () => this.highlightConnections(null, edge.id), { signal: this.graphAbort.signal });
      group.addEventListener("pointerleave", () => this.highlightConnections(null), { signal: this.graphAbort.signal });
      fragment.append(group);
      this.edges.set(edge.id, { group, source: edge.source, target: edge.target });
      for (const id of [edge.source, edge.target]) {
        let incident = this.incidentEdges.get(id);
        if (!incident) this.incidentEdges.set(id, incident = new Set());
        incident.add(edge.id);
      }
    }
    for (const [index, node] of this.layout.nodes.entries()) {
      const preview = node.preview;
      const portsLabel = node.ports ? `, inputs: ${node.ports.inputs.map((port) => `${displayPortName(port.name)} (${port.type}, ${port.kind})`).join(", ") || "none"}, outputs: ${node.ports.outputs.map((port) => `${displayPortName(port.name)} (${port.type}, ${port.kind})`).join(", ") || "none"}` : node.pending ? "" : ", port metadata not recorded";
      const scopeLabel = node.kind === "workflow" ? `, imported workflow, ${node.child_count ?? 0} steps, ${node.failure_count ?? 0} failed` : "";
      const label = `${node.title}, ${node.state}${scopeLabel}${portsLabel}${preview ? `, ${preview.label}, ${preview.count} image ${preview.count === 1 ? "output" : "outputs"}` : ""}`;
      const group = element("g", { transform: `translate(${node.x},${node.y})`, class: `fx-canvas-node${node.kind === "workflow" ? " fx-canvas-workflow" : node.kind === "boundary" ? " fx-canvas-boundary" : ""}`, role: node.kind === "workflow" ? "group" : "button", tabindex: 0, "data-node-id": node.id, "aria-label": label, ...(node.kind === "workflow" ? { "aria-current": "false" } : { "aria-pressed": "false" }) });
      group.append(element("title", {}, `${node.title}\n${node.subtitle}\n${node.source_id}`));
      group.append(element("rect", { width: node.width, height: node.height, rx: 11, class: node.pending ? "fx-canvas-node-box fx-canvas-pending" : "fx-canvas-node-box" }));
      group.append(element("text", { x: 16, y: 27, class: "fx-canvas-node-title" }, short(node.title, node.ports ? 42 : 28)));
      group.append(element("text", { x: 16, y: 47, class: "fx-canvas-node-type" }, short(node.kind === "workflow" ? node.path ?? node.subtitle : node.subtitle, node.ports ? 51 : 34)));
      if (node.ports) {
        group.append(element("line", { x1: 0, y1: 59, x2: node.width, y2: 59, class: "fx-canvas-node-divider" }));
        group.append(element("text", { x: 16, y: 76, class: "fx-canvas-port-heading" }, "INPUTS"));
        group.append(element("text", { x: node.width - 16, y: 76, "text-anchor": "end", class: "fx-canvas-port-heading" }, "OUTPUTS"));
        for (const direction of ["input", "output"] as const) {
          const ports = direction === "input" ? node.ports.inputs : node.ports.outputs;
          const x = direction === "input" ? 0 : node.width;
          const textX = direction === "input" ? 16 : node.width - 16;
          for (const [row, port] of ports.entries()) {
            const y = PORT_TOP + row * PORT_ROW;
            const socket = element("g", { class: `fx-canvas-port fx-canvas-port-${port.kind}`, "data-port-name": port.name, "data-port-direction": direction, "data-port-kind": port.kind });
            socket.append(element("title", {}, `${direction}: ${displayPortName(port.name)} · ${port.type} · ${port.kind}`));
            socket.append(port.kind === "artifact" ? element("circle", { cx: x, cy: y, r: 4.5 })
              : port.kind === "fact" ? element("path", { d: `M${x} ${y - 5}l5 5l-5 5l-5 -5z` })
              : element("rect", { x: x - 4, y: y - 4, width: 8, height: 8, rx: 1 }));
            socket.append(element("text", { x: textX, y: y + 3, "text-anchor": direction === "input" ? "start" : "end", class: "fx-canvas-port-name" }, short(displayPortName(port.name), 22)));
            socket.append(element("text", { x: textX, y: y + 16, "text-anchor": direction === "input" ? "start" : "end", class: "fx-canvas-port-type" }, short(`${port.kind === "parameter" ? "param · " : ""}${port.type}`, 25)));
            group.append(socket);
          }
          if (!ports.length) group.append(element("text", { x: textX, y: PORT_TOP + 3, "text-anchor": direction === "input" ? "start" : "end", class: "fx-canvas-port-type" }, "None"));
        }
        if (node.ports.settings.length) {
          const settings = element("text", { x: 16, y: canvasNodeGeometry(node).settings_y + 10, class: "fx-canvas-settings" }, short(`Settings · ${node.ports.settings.map((setting) => setting.name).join(", ")}`, 52));
          settings.append(element("title", {}, node.ports.settings.map((setting) => `${setting.name}: ${setting.type}`).join("\n")));
          group.append(settings);
        }
      } else if (!node.pending && !preview) {
        group.append(element("text", { x: 16, y: 64, class: "fx-canvas-port-type" }, "Port metadata not recorded"));
      }
      if (preview) {
        const frame = { x: 16, y: canvasNodeGeometry(node).preview_y, width: node.width - 32, height: 140 };
        const clipId = `${this.id}-preview-${index}`;
        const clip = element("clipPath", { id: clipId });
        clip.append(element("rect", { ...frame, rx: 6 }));
        group.append(clip, element("rect", { ...frame, rx: 6, fill: `url(#${this.id}-checker)` }));
        const image = element("image", { ...frame, preserveAspectRatio: "xMidYMid meet", "clip-path": `url(#${clipId})`, role: "img", "aria-label": preview.label, class: "fx-canvas-node-image" });
        image.append(element("title", {}, preview.label));
        const fallback = element("text", { x: node.width / 2, y: frame.y + frame.height / 2, "text-anchor": "middle", "dominant-baseline": "middle", class: "fx-canvas-image-error", visibility: "hidden" }, "Image unavailable");
        image.addEventListener("error", () => {
          image.setAttribute("visibility", "hidden");
          fallback.setAttribute("visibility", "visible");
          group.setAttribute("aria-label", `${node.title}, ${node.state}${portsLabel}, Image unavailable: ${preview.label}`);
        }, { signal: this.graphAbort.signal });
        image.setAttribute("href", preview.url);
        group.append(image, element("rect", { ...frame, rx: 6, class: "fx-canvas-image-border" }), fallback);
        group.append(element("text", { x: node.width - 16, y: node.height - 22, "text-anchor": "end", class: "fx-canvas-image-count" }, `${preview.count} image ${preview.count === 1 ? "output" : "outputs"}`));
      }
      group.append(element("text", { x: 16, y: node.height - 22, class: "fx-canvas-node-state" }, node.state));
      if (node.kind === "workflow" && node.scope_id) {
        group.append(element("text", { x: 16, y: node.height - 48, class: (node.failure_count ?? 0) > 0 ? "fx-canvas-workflow-failure" : "fx-canvas-workflow-count" }, `${node.child_count ?? 0} steps${node.failure_count ? ` · ${node.failure_count} failed` : ""}`));
        const open = element("g", { class: "fx-canvas-open-workflow", role: "button", tabindex: 0, "aria-label": `Open workflow ${node.title}` });
        open.append(element("rect", { x: node.width - 144, y: node.height - 37, width: 128, height: 24, rx: 5 }));
        open.append(element("text", { x: node.width - 80, y: node.height - 21, "text-anchor": "middle" }, "Open workflow →"));
        const enter = () => this.onOpenScope(node.scope_id!);
        open.addEventListener("click", (event) => { event.stopPropagation(); enter(); }, { signal: this.graphAbort.signal });
        open.addEventListener("keydown", (event) => {
          if (event.key === "Enter" || event.key === " ") { event.preventDefault(); event.stopPropagation(); enter(); }
        }, { signal: this.graphAbort.signal });
        group.addEventListener("dblclick", enter, { signal: this.graphAbort.signal });
        group.append(open);
      }
      group.addEventListener("pointerenter", () => this.highlightConnections(node.id), { signal: this.graphAbort.signal });
      group.addEventListener("pointerleave", () => this.highlightConnections(null), { signal: this.graphAbort.signal });
      group.addEventListener("click", () => this.onSelect(node.id), { signal: this.graphAbort.signal });
      group.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") { event.preventDefault(); this.onSelect(node.id); }
      }, { signal: this.graphAbort.signal });
      fragment.append(group);
      this.nodes.set(node.id, group);
    }
    this.content.replaceChildren(fragment);
    this.empty.hidden = this.layout.nodes.length > 0;
    const selected = this.selected;
    this.selected = null;
    this.setSelection(selected);
    if (!this.didFit) this.fit();
    else this.applyViewport();
    if (navigating || restoringFocus) this.container.focus({ preventScroll: true });
  }
  setSelection(id: string | null) {
    if (id === this.selected) return;
    const previous = this.selected;
    this.selected = id;
    for (const key of [previous, id]) {
      if (key === null) continue;
      const group = this.nodes.get(key);
      if (!group) continue;
      group.classList.toggle("is-selected", key === id);
      group.setAttribute(group.getAttribute("role") === "button" ? "aria-pressed" : "aria-current", String(key === id));
    }
  }
  private highlightConnections(nodeId: string | null, edgeId: string | null = null) {
    const next = this.drag ? new Set<string>() : nodeId !== null
      ? this.incidentEdges.get(nodeId) ?? new Set<string>()
      : new Set(edgeId === null ? [] : [edgeId]);
    for (const id of this.highlightedEdges) if (!next.has(id)) this.edges.get(id)?.group.classList.remove("is-highlighted");
    for (const id of next) if (!this.highlightedEdges.has(id)) this.edges.get(id)?.group.classList.add("is-highlighted");
    this.highlightedEdges = next;
  }
  fit() {
    const bounds = this.container.getBoundingClientRect();
    this.viewport.fit(this.layout, bounds);
    this.didFit = bounds.width > 0 && bounds.height > 0 && this.layout.nodes.length > 0;
    this.applyViewport();
  }
  private zoom(factor: number) {
    const bounds = this.container.getBoundingClientRect();
    this.viewport.zoomAt(factor, { x: bounds.width / 2, y: bounds.height / 2 });
    this.applyViewport();
  }
  private applyViewport() {
    if (this.viewportFrame !== null) cancelAnimationFrame(this.viewportFrame);
    this.viewportFrame = null;
    const { x, y, zoom } = this.viewport.getSnapshot();
    this.content.setAttribute("transform", `translate(${x},${y}) scale(${zoom})`);
    this.zoomLabel.textContent = `${Math.round(zoom * 100)}%`;
  }
  /** Keep every input delta, but write camera DOM only once per animation frame. */
  private scheduleViewport() {
    if (this.viewportFrame !== null) return;
    this.viewportFrame = requestAnimationFrame(() => {
      this.viewportFrame = null;
      this.applyViewport();
    });
  }
  private onWheel = (event: WheelEvent) => {
    event.preventDefault();
    const bounds = this.svg.getBoundingClientRect();
    this.viewport.wheel(event, { x: event.clientX - bounds.left, y: event.clientY - bounds.top }, bounds);
    this.scheduleViewport();
  };
  private onPointerDown = (event: PointerEvent) => {
    if (event.button !== 0 || (event.target as Element).closest("[data-node-id]")) return;
    this.drag = { pointer: event.pointerId, x: event.clientX, y: event.clientY, startX: event.clientX, startY: event.clientY, moved: false, background: !(event.target as Element).closest("[data-connection-id]") };
    this.highlightConnections(null);
    this.svg.setPointerCapture(event.pointerId);
    this.container.classList.add("is-panning");
  };
  private onPointerMove = (event: PointerEvent) => {
    if (!this.drag || event.pointerId !== this.drag.pointer) return;
    this.drag.moved ||= Math.hypot(event.clientX - this.drag.startX, event.clientY - this.drag.startY) > 4;
    this.viewport.pan(event.clientX - this.drag.x, event.clientY - this.drag.y);
    this.drag.x = event.clientX; this.drag.y = event.clientY;
    this.scheduleViewport();
  };
  private onPointerUp = (event: PointerEvent) => {
    if (!this.drag || this.drag.pointer !== event.pointerId) return;
    const clear = event.type === "pointerup" && this.drag.background && !this.drag.moved
      && Math.hypot(event.clientX - this.drag.startX, event.clientY - this.drag.startY) <= 4;
    this.cancelDrag();
    if (clear) {
      this.container.focus({ preventScroll: true });
      this.setSelection(null);
      this.onSelect(null);
    }
  };
  private cancelDrag() {
    if (this.drag && this.svg.hasPointerCapture(this.drag.pointer)) this.svg.releasePointerCapture(this.drag.pointer);
    this.drag = null;
    this.container.classList.remove("is-panning");
  }
  dispose() {
    if (this.viewportFrame !== null) cancelAnimationFrame(this.viewportFrame);
    this.viewportFrame = null;
    this.cancelDrag(); this.abort.abort(); this.graphAbort.abort(); this.resize.disconnect(); this.container.replaceChildren();
  }
}
