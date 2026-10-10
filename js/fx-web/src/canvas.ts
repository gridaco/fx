import type { CanvasGraph, LayoutReport } from "./graph";
import { CanvasViewport } from "./controller";
import { displayPortName } from "./ports";
import { canvasEdgePath, layoutGraph, type CanvasLayout, type PlacedDeck, type PositionedFrame, type PositionedNode } from "./layout";
import { deckBadge, expandedColumns, restingFront, type DeckMember } from "./deck";
import { element, nodeCard, runningLabel, short } from "./card";

const EXPANDED_GAP = 32;
/** §5.9: expanded decks fit the viewport down to card titles (13 px) about 11 px on screen. */
const LEGIBLE_ZOOM = 11 / 13;

interface Deck {
  placed: PlacedDeck;
  title: string;
  parent: SVGGElement;
  /** Member cards; a frame card holds its nodes' cards. */
  cards: SVGGElement[];
  boxes: { x: number; y: number; width: number; height: number }[];
  members: DeckMember[];
  badge: SVGGElement;
}
interface OpenDeck { deck: Deck; veil: SVGRectElement; camera: ReturnType<CanvasViewport["getSnapshot"]> }

/** Imperative, read-only SVG surface. DOM listeners, selection, decks and camera live here. */
export class CanvasController {
  private static nextId = 0;
  private readonly id = `fx-canvas-${++CanvasController.nextId}`;
  private readonly svg = element("svg", { role: "group", "aria-label": "Workflow dependency canvas" });
  private readonly content = element("g");
  private readonly viewport = new CanvasViewport();
  private readonly nodes = new Map<string, SVGGElement>();
  private readonly frameCards = new Map<string, SVGGElement>();
  private readonly edges = new Map<string, { group: SVGGElement; source: string; target: string }>();
  private readonly incidentEdges = new Map<string, Set<string>>();
  private highlightedEdges = new Set<string>();
  private decks: Deck[] = [];
  private open: OpenDeck | null = null;
  private hovered: { deck: number; card: number } | null = null;
  private viewportFrame: number | null = null;
  private readonly abort = new AbortController();
  private graphAbort = new AbortController();
  private readonly resize: ResizeObserver;
  private readonly toolbar = document.createElement("div");
  private readonly zoomLabel = document.createElement("span");
  private readonly empty = document.createElement("p");
  private readonly deckBar = document.createElement("div");
  private layout: CanvasLayout = { nodes: [], edges: [], frames: [], decks: [], width: 0, height: 0 };
  private readonly canHover = window.matchMedia("(hover: hover)");
  private readonly reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  private selected: string | null = null;
  private drag: { pointer: number; x: number; y: number; startX: number; startY: number; moved: boolean; background: boolean; veil: boolean } | null = null;
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
    this.content.style.transformOrigin = "0 0";
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
    this.deckBar.className = "fx-deck-bar";
    this.deckBar.hidden = true;
    container.replaceChildren(this.svg, this.toolbar, this.empty, this.deckBar);
    container.addEventListener("wheel", this.onWheel, { passive: false, signal: this.abort.signal });
    this.svg.addEventListener("pointerdown", this.onPointerDown, { signal: this.abort.signal });
    this.svg.addEventListener("pointermove", this.onPointerMove, { signal: this.abort.signal });
    this.svg.addEventListener("pointerup", this.onPointerUp, { signal: this.abort.signal });
    this.svg.addEventListener("pointercancel", this.onPointerUp, { signal: this.abort.signal });
    this.svg.addEventListener("pointerleave", () => this.hover(null), { signal: this.abort.signal });
    // A deck card is the expand target: the capture phase runs before a card's own selection.
    this.svg.addEventListener("click", this.onDeckClick, { capture: true, signal: this.abort.signal });
    // Raising a card moves focus out of the canvas, so Esc listens on the document while open.
    document.addEventListener("keydown", (event) => {
      if (event.key !== "Escape" || !this.open || event.defaultPrevented) return;
      const target = event.target as HTMLElement | null;
      if (target?.closest("input, textarea, select, [contenteditable]")) return;
      event.preventDefault();
      this.collapse();
    }, { capture: true, signal: this.abort.signal });
    container.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.scope !== null && !event.defaultPrevented) { event.preventDefault(); this.onBack(); }
    }, { signal: this.abort.signal });
    this.resize = new ResizeObserver(() => { if (!this.didFit && this.layout.nodes.length) this.fit(); });
    this.resize.observe(container);
    this.applyViewport();
  }

  setGraph(graph: CanvasGraph, scope: string | null = null, report: LayoutReport | null = null) {
    const navigating = scope !== this.scope;
    const restoringFocus = this.content.contains(document.activeElement);
    const reopen = navigating || !this.open ? null : { id: this.open.deck.placed.id, camera: this.open.camera };
    this.cancelDrag();
    this.collapse(true, !navigating);
    if (navigating) {
      this.cameras.set(this.scope, { viewport: this.viewport.getSnapshot(), didFit: this.didFit });
      this.scope = scope;
      const camera = this.cameras.get(scope);
      this.didFit = camera?.didFit ?? false;
      if (camera) this.viewport.restore(camera.viewport);
    }
    this.graphAbort.abort();
    this.graphAbort = new AbortController();
    this.layout = layoutGraph(graph, report);
    this.nodes.clear();
    this.frameCards.clear();
    this.edges.clear();
    this.incidentEdges.clear();
    this.highlightedEdges.clear();
    this.hovered = null;
    this.render();
    this.empty.hidden = this.layout.nodes.length > 0;
    const selected = this.selected;
    this.selected = null;
    this.setSelection(selected);
    if (!this.didFit) this.fit();
    else this.applyViewport();
    // A live update keeps an open deck open, and closing it still returns to the camera before it opened.
    if (reopen !== null) {
      const index = this.decks.findIndex((deck) => deck.placed.id === reopen.id);
      if (index >= 0) { this.expand(index, true); this.open!.camera = reopen.camera; }
      // A deck that vanished or dropped below two members stays closed and gives the camera back.
      else this.animateCamera(() => this.viewport.restore(reopen.camera));
    }
    if (navigating || restoringFocus) this.container.focus({ preventScroll: true });
  }

  /** Frames nest as groups, so a frame card carries its steps and inner connections. */
  private render() {
    const signal = this.graphAbort.signal;
    const frames = this.layout.frames;
    const frameOf = new Map<string, string>();
    for (const frame of frames) for (const id of frame.nodes) frameOf.set(id, frame.id);
    const parentFrame = new Map(frames.map((frame) => [frame.id, frame.parent ?? null]));
    // A frame and its ancestors, innermost first, ending at the root.
    const chain = (id: string | null) => {
      const ids: (string | null)[] = [];
      for (let at = id; at !== null && !ids.includes(at); at = parentFrame.get(at) ?? null) ids.push(at);
      ids.push(null);
      return ids;
    };
    const layers = new Map<string | null, { edges: SVGGElement; cards: SVGGElement }>();
    const layer = (frame: string | null) => layers.get(frame)!;
    const rootEdges = element("g", { class: "fx-canvas-edges" }), rootCards = element("g", { class: "fx-canvas-cards" });
    layers.set(null, { edges: rootEdges, cards: rootCards });
    for (const frame of frames) {
      const card = this.frameCard(frame);
      const edges = element("g", { class: "fx-canvas-edges" }), cards = element("g", { class: "fx-canvas-cards" });
      card.append(edges, cards);
      layers.set(frame.id, { edges, cards });
      this.frameCards.set(frame.id, card);
    }
    for (const frame of frames) layer(frame.parent && layers.has(frame.parent) ? frame.parent : null).cards.append(this.frameCards.get(frame.id)!);
    for (const edge of this.layout.edges) {
      const from = chain(frameOf.get(edge.source) ?? null), to = chain(frameOf.get(edge.target) ?? null);
      let common: string | null = null;
      for (const id of from) if (to.includes(id)) { common = id; break; }
      const group = this.edgeElement(edge);
      layer(common).edges.append(group);
      this.edges.set(edge.id, { group, source: edge.source, target: edge.target });
      for (const id of [edge.source, edge.target]) {
        let incident = this.incidentEdges.get(id);
        if (!incident) this.incidentEdges.set(id, incident = new Set());
        incident.add(edge.id);
      }
    }
    for (const [index, node] of this.layout.nodes.entries()) {
      const group = nodeCard(node, this.id, index, signal, (scope) => this.onOpenScope(scope));
      group.addEventListener("pointerenter", () => this.highlightConnections(node.id), { signal });
      group.addEventListener("pointerleave", () => this.highlightConnections(null), { signal });
      group.addEventListener("click", () => this.onSelect(node.id), { signal });
      group.addEventListener("keydown", (event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        const hit = this.cardAt(group);
        if (hit && !this.open) this.expand(hit.deck);
        else this.onSelect(node.id);
      }, { signal });
      layer(frameOf.get(node.id) ?? null).cards.append(group);
      this.nodes.set(node.id, group);
    }
    const items = new Map<string, PositionedNode | PositionedFrame>([...this.layout.nodes, ...frames].map((item) => [item.id, item]));
    this.decks = this.layout.decks.map((placed, index) => this.deck(placed, index, layer(placed.frame).cards, items));
    this.decks.forEach((_, index) => this.restDeck(index));
    this.content.replaceChildren(rootEdges, rootCards);
    // Running cards count up together, once a second, until the next redraw.
    const clocks = [...this.content.querySelectorAll<SVGTextElement>("text[data-started]")];
    if (clocks.length) {
      const timer = window.setInterval(() => {
        const now = Date.now();
        for (const clock of clocks) clock.textContent = runningLabel(Number(clock.dataset.started), now);
      }, 1000);
      signal.addEventListener("abort", () => window.clearInterval(timer), { once: true });
    }
  }

  private frameCard(frame: PositionedFrame) {
    const group = element("g", { class: "fx-canvas-frame", "data-frame-id": frame.id, "aria-label": frame.title });
    group.append(element("rect", { x: frame.x, y: frame.y, width: frame.width, height: frame.height, rx: 12, class: "fx-canvas-frame-box" }));
    group.append(element("text", { x: frame.x + 14, y: frame.y + 16 }, short(frame.title, 64)));
    return group;
  }

  private edgeElement(edge: CanvasLayout["edges"][number]) {
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
    return group;
  }

  /** §5.3: the cards of one slot in one frame, a count badge above them. */
  private deck(placed: PlacedDeck, index: number, parent: SVGGElement, items: Map<string, PositionedNode | PositionedFrame>): Deck {
    const cards: SVGGElement[] = [];
    const boxes: Deck["boxes"] = [];
    for (const id of placed.members) {
      const item = items.get(id)!;
      boxes.push({ x: item.x, y: item.y, width: item.width, height: item.height });
      const card = this.nodes.get(id) ?? this.frameCards.get(id)!;
      card.classList.add("fx-deck-card");
      card.dataset.deck = String(index);
      card.dataset.deckCard = String(cards.length);
      cards.push(card);
    }
    // §5.4 rule 4: a member that an instance outside the deck reads.
    const inside = (id: string) => { const node = this.nodes.get(id); return node ? cards.findIndex((card) => card.contains(node)) : -1; };
    const bound = new Set<number>();
    for (const edge of this.edges.values()) {
      const from = inside(edge.source);
      if (from >= 0 && inside(edge.target) < 0) bound.add(from);
    }
    const members: DeckMember[] = placed.members.map((id, at) => {
      const item = items.get(id)!;
      return { id, state: item.state ?? "planned", key: item.key ?? null, take: item.take?.at(-1) ?? null, bound: bound.has(at) };
    });
    const title = items.get(placed.members[0])?.title ?? placed.address;
    const badge = deckBadge(members);
    const label = [badge.text, ...badge.detail].join(" · ");
    const group = element("g", { class: `fx-deck-badge${badge.failed ? " has-failures" : ""}`, role: "button", tabindex: 0, "data-deck": index, "aria-label": `Expand ${title}, ${label}` });
    const width = 16 + 7 * label.length;
    group.append(element("rect", { x: placed.x, y: placed.y, width, height: 22, rx: 11 }));
    group.append(element("text", { x: placed.x + 10, y: placed.y + 15, class: "fx-deck-badge-full" }, label));
    group.append(element("text", { x: placed.x + 10, y: placed.y + 15, class: "fx-deck-badge-short" }, badge.short));
    group.append(element("circle", { cx: placed.x + 11, cy: placed.y + 11, r: 5, class: "fx-deck-badge-dot" }));
    group.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") { event.preventDefault(); this.expand(index); }
    }, { signal: this.graphAbort.signal });
    parent.append(group);
    return { placed, title, parent, cards, boxes, members, badge: group };
  }
  /** The deck card that is or holds this node's card, or -1. */
  private holding(deck: Deck, id: string) {
    const node = this.nodes.get(id);
    return node ? deck.cards.findIndex((card) => card.contains(node)) : -1;
  }

  /** §5.4: the resting front card goes last in its layer, the badge above it. Returns the front. */
  private restDeck(index: number, front = restingFront(this.decks[index].members, this.selectedMember(index))) {
    const deck = this.decks[index];
    if (this.open?.deck === deck) return null;
    const order = deck.cards.filter((_, card) => card !== front);
    for (const card of [...order, deck.cards[front]]) deck.parent.append(card);
    deck.parent.append(deck.badge);
    // One tab stop per closed deck: its front card.
    deck.cards.forEach((card, at) => { if (card.hasAttribute("tabindex")) card.setAttribute("tabindex", at === front ? "0" : "-1"); });
    return deck.cards[front];
  }
  private selectedMember(index: number) {
    if (this.selected === null) return null;
    const deck = this.decks[index];
    const at = this.holding(deck, this.selected);
    return at >= 0 ? deck.members[at].id : null;
  }

  private cardAt(target: EventTarget | null): { deck: number; card: number } | null {
    const card = (target as Element | null)?.closest?.<SVGGElement>(".fx-deck-card");
    if (!card || card.dataset.deck === undefined) return null;
    return { deck: Number(card.dataset.deck), card: Number(card.dataset.deckCard) };
  }
  /** Hover raises the card under the pointer; leaving it restores the resting card. */
  private hover(target: { deck: number; card: number } | null) {
    if (this.open || !this.canHover.matches) target = null;
    const previous = this.hovered;
    if (previous?.deck === target?.deck && previous?.card === target?.card) return;
    this.hovered = target;
    if (previous && previous.deck !== target?.deck) this.restDeck(previous.deck);
    if (target) this.restDeck(target.deck, target.card);
  }
  private onDeckClick = (event: MouseEvent) => {
    // A labelled control on a card does what it says (§5.7).
    if ((event.target as Element).closest(".fx-canvas-open-workflow")) return;
    const badge = (event.target as Element).closest<SVGGElement>(".fx-deck-badge");
    const hit = badge ? { deck: Number(badge.dataset.deck), card: 0 } : this.cardAt(event.target);
    if (!hit || this.open) return;
    event.stopImmediatePropagation();
    event.preventDefault();
    this.expand(hit.deck);
  };

  private get motion() { return this.reducedMotion.matches ? 0 : 220; }

  /** §5.7: the deck spreads into a grid in place over a veil; the camera fits the grid. */
  private expand(index: number, instant = false) {
    const deck = this.decks[index];
    if (!deck || this.open) return;
    this.hover(null);
    const sizes = deck.boxes;
    const card = { width: Math.max(...sizes.map((size) => size.width)), height: Math.max(...sizes.map((size) => size.height)) };
    const bounds = this.container.getBoundingClientRect();
    const columns = expandedColumns(sizes.length, card, bounds);
    const origin = { x: deck.placed.x, y: deck.placed.y };
    const veil = element("rect", { x: -1e6, y: -1e6, width: 2e6, height: 2e6, class: "fx-deck-veil" });
    this.content.append(veil, ...deck.cards);
    const camera = this.viewport.getSnapshot();
    const duration = instant ? 0 : this.motion;
    // Moved cards start their transition from where they rest.
    this.content.getBoundingClientRect();
    deck.cards.forEach((element, at) => {
      if (element.hasAttribute("tabindex")) element.setAttribute("tabindex", "0");
      const x = origin.x + (at % columns) * (card.width + EXPANDED_GAP), y = origin.y + Math.floor(at / columns) * (card.height + EXPANDED_GAP);
      element.style.transition = `transform ${duration}ms ease`;
      // A CSS transform replaces a node card's own transform attribute; a frame card has none.
      element.style.transform = element.hasAttribute("transform") ? `translate(${x}px, ${y}px)` : `translate(${x - sizes[at].x}px, ${y - sizes[at].y}px)`;
    });
    veil.style.transition = `opacity ${duration}ms ease`;
    veil.classList.add("is-shown");
    this.open = { deck, veil, camera };
    this.deckBar.replaceChildren();
    const heading = document.createElement("span");
    heading.textContent = `${deck.title} · ${deck.members.length} · Esc or click outside to close`;
    const close = document.createElement("button");
    close.type = "button";
    close.textContent = "×";
    close.setAttribute("aria-label", "Close");
    close.addEventListener("click", () => this.collapse(), { signal: this.graphAbort.signal });
    this.deckBar.append(heading, close);
    this.deckBar.hidden = false;
    if (instant) return;
    const rows = Math.ceil(sizes.length / columns);
    this.animateCamera(() => {
      this.viewport.fit({ x: origin.x, y: origin.y, width: columns * (card.width + EXPANDED_GAP) - EXPANDED_GAP, height: rows * (card.height + EXPANDED_GAP) - EXPANDED_GAP }, bounds);
      const fitted = this.viewport.getSnapshot();
      if (fitted.zoom < LEGIBLE_ZOOM) this.viewport.restore({ zoom: LEGIBLE_ZOOM, x: 48 - origin.x * LEGIBLE_ZOOM, y: 64 - origin.y * LEGIBLE_ZOOM });
    });
  }

  /**
   * Slides the cards back; the selected member rests in front. A collapse the viewer asks for
   * (Esc, ×, the veil) returns focus to the front card; an instant one, from an update or a
   * selection, leaves focus where it is.
   */
  private collapse(instant = false, keepCamera = false) {
    const open = this.open;
    if (!open) return;
    this.open = null;
    this.deckBar.hidden = true;
    const duration = instant ? 0 : this.motion;
    for (const card of open.deck.cards) { card.style.transition = `transform ${duration}ms ease`; card.style.transform = ""; }
    open.veil.style.transition = `opacity ${duration}ms ease`;
    open.veil.classList.remove("is-shown");
    if (!keepCamera) this.animateCamera(() => this.viewport.restore(open.camera), duration);
    const settle = () => {
      open.veil.remove();
      for (const card of open.deck.cards) card.style.transition = "";
      // An update may have replaced the decks meanwhile.
      const index = this.decks.indexOf(open.deck);
      const front = index >= 0 ? this.restDeck(index) : null;
      if (!instant && front) (front.hasAttribute("tabindex") ? front : this.container).focus({ preventScroll: true });
    };
    if (duration === 0) settle();
    else window.setTimeout(settle, duration);
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
    // Selecting from outside an open deck closes it; a closed deck raises the selected member.
    if (this.open && id !== null && this.holding(this.open.deck, id) < 0) this.collapse(true);
    this.decks.forEach((deck, index) => {
      if ([previous, id].some((key) => key !== null && this.holding(deck, key) >= 0)) this.restDeck(index);
    });
  }
  private highlightConnections(nodeId: string | null, edgeId: string | null = null) {
    const next = this.drag?.moved ? new Set<string>() : nodeId !== null
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
  /** A programmatic camera move glides; input moves stay immediate. */
  private animateCamera(move: () => void, duration = this.motion) {
    this.content.style.transition = duration ? `transform ${duration}ms ease` : "";
    move();
    this.applyViewport();
    if (duration) window.setTimeout(() => { this.content.style.transition = ""; }, duration);
  }
  private applyViewport() {
    if (this.viewportFrame !== null) cancelAnimationFrame(this.viewportFrame);
    this.viewportFrame = null;
    const { x, y, zoom } = this.viewport.getSnapshot();
    this.content.style.transform = `translate(${x}px, ${y}px) scale(${zoom})`;
    // §5.3: a badge too small to read shows its count, then only a failure dot.
    this.content.classList.toggle("is-zoom-small", zoom * 13 < 9);
    this.content.classList.toggle("is-zoom-tiny", zoom * 13 < 6);
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
    this.content.style.transition = "";
    const bounds = this.svg.getBoundingClientRect();
    this.viewport.wheel(event, { x: event.clientX - bounds.left, y: event.clientY - bounds.top }, bounds);
    this.scheduleViewport();
  };
  /**
   * §5.7: a press anywhere but a labelled control may pan. Past 4 px it captures the pointer and
   * pans, so its release clicks nothing; a still press stays a click for cards and badges.
   */
  private onPointerDown = (event: PointerEvent) => {
    const target = event.target as Element;
    if (event.button !== 0 || target.closest(".fx-canvas-open-workflow")) return;
    this.drag = { pointer: event.pointerId, x: event.clientX, y: event.clientY, startX: event.clientX, startY: event.clientY, moved: false,
      background: !target.closest("[data-node-id], [data-connection-id], .fx-deck-badge, .fx-deck-card"), veil: Boolean(target.closest(".fx-deck-veil")) };
  };
  private onPointerMove = (event: PointerEvent) => {
    if (!this.drag) { this.hover(this.cardAt(event.target)); return; }
    if (event.pointerId !== this.drag.pointer) return;
    if (!this.drag.moved) {
      if (Math.hypot(event.clientX - this.drag.startX, event.clientY - this.drag.startY) <= 4) return;
      this.drag.moved = true;
      this.content.style.transition = "";
      this.highlightConnections(null);
      this.svg.setPointerCapture(event.pointerId);
      this.container.classList.add("is-panning");
    }
    this.viewport.pan(event.clientX - this.drag.x, event.clientY - this.drag.y);
    this.drag.x = event.clientX; this.drag.y = event.clientY;
    this.scheduleViewport();
  };
  private onPointerUp = (event: PointerEvent) => {
    if (!this.drag || this.drag.pointer !== event.pointerId) return;
    const { moved, background, veil } = this.drag;
    this.cancelDrag();
    // Cards and badges answer a still press through their click events.
    if (moved || event.type !== "pointerup") return;
    if (this.open && veil) { this.collapse(); return; }
    if (background) {
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
