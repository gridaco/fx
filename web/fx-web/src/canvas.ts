import { createElement, X } from "lucide";
import type { CanvasGraph, LayoutReport } from "./graph";
import { CanvasViewport, type CanvasInset, type ViewportState } from "./controller";
import { displayPortName } from "./ports";
import { BADGE_BAND, DECK_OFFSET, canvasEdgePath, layoutGraph, type CanvasLayout, type PlacedDeck, type PositionedFrame, type PositionedNode } from "./layout";
import { DECK_BACK, bookmarkLayout, deckBadge, deckStack, deckTicks, expandedColumns, restingFront, tickHolds, type DeckMember } from "./deck";
import { element, nodeCard, runningLabel, short } from "./card";

const EXPANDED_GAP = 32;
/** §5.9: expanded decks fit the viewport down to card titles (13 px) about 11 px on screen. */
const LEGIBLE_ZOOM = 11 / 13;

/**
 * The bookmark's geometry, in canvas units: its gap to the deck, its marks' lengths, their pitch,
 * and the band it keeps to, beside the card's title above its sockets and the wires they take.
 */
const BOOKMARK_GAP = 6, BOOKMARK_SHORT = 7, BOOKMARK_LONG = 14, BOOKMARK_STEP = 8, BOOKMARK_BAND = 72;
/** How far a bookmark's rows reach past its marks: left, above the first and below the last. */
const BOOKMARK_REACH = 12;
/** The open deck's bookmark, in screen pixels: its marks' pitch and the most they take. */
const SCRUBBER_STEP = 10, SCRUBBER_BAND = 240;

/** The cell of an expanded deck's grid that holds its member at `at`. */
function cell(grid: OpenDeck["grid"], at: number) {
  return { x: grid.x + (at % grid.columns) * (grid.width + EXPANDED_GAP), y: grid.y + Math.floor(at / grid.columns) * (grid.height + EXPANDED_GAP) };
}
/** Puts a deck card's corner at `to`, or back where the layout put it (`box`) with null. */
function place(card: SVGGElement, box: { x: number; y: number }, to: { x: number; y: number } | null) {
  // A CSS transform replaces a node card's own transform attribute; a frame card has none.
  card.style.transform = !to ? "" : card.hasAttribute("transform") ? `translate(${to.x}px, ${to.y}px)` : `translate(${to.x - box.x}px, ${to.y - box.y}px)`;
}

interface Deck {
  placed: PlacedDeck;
  title: string;
  parent: SVGGElement;
  /** The bookmark beside the deck (§5.3), and what each member is called on it. */
  bookmark: SVGGElement;
  names: string[];
  /** Member cards; a frame card holds its nodes' cards. */
  cards: SVGGElement[];
  boxes: { x: number; y: number; width: number; height: number }[];
  members: DeckMember[];
  badge: SVGGElement;
  /** The first member the closed stack shows (§5.3). */
  start: number;
}
/** A deck card under the pointer; `mark` when it is pointed at from the bookmark. */
type DeckHit = { deck: number; card: number; mark?: boolean };
interface OpenDeck {
  deck: Deck;
  veil: SVGRectElement;
  camera: ReturnType<CanvasViewport["getSnapshot"]>;
  /** The grid the cards spread into: its corner, columns and cell. */
  grid: { x: number; y: number; columns: number; width: number; height: number };
}

/** Imperative, read-only SVG surface. DOM listeners, selection, decks and camera live here. */
export class CanvasController {
  private static nextId = 0;
  private readonly id = `fx-canvas-${++CanvasController.nextId}`;
  private readonly svg = element("svg", { role: "group", "aria-label": "Workflow dependency canvas" });
  private readonly content = element("g");
  private readonly viewport = new CanvasViewport();
  private readonly nodes = new Map<string, SVGGElement>();
  private readonly frameCards = new Map<string, SVGGElement>();
  private readonly edges = new Map<string, { group: SVGGElement; source: string; target: string; points: CanvasLayout["edges"][number]["points"] }>();
  /** How far the stack moved each deck card from where the layout put it, down and right. */
  private readonly shifts = new Map<SVGGElement, number>();
  private readonly incidentEdges = new Map<string, Set<string>>();
  private highlightedEdges = new Set<string>();
  private decks: Deck[] = [];
  private open: OpenDeck | null = null;
  private hovered: DeckHit | null = null;
  /** The open deck's card a bookmark mark points at. */
  private marked: SVGGElement | null = null;
  private viewportFrame: number | null = null;
  private readonly abort = new AbortController();
  private graphAbort = new AbortController();
  private readonly resize: ResizeObserver;
  private readonly toolbar = document.createElement("div");
  private readonly zoomLabel = document.createElement("span");
  private readonly empty = document.createElement("p");
  private readonly deckBar = document.createElement("div");
  /** An open deck's bookmark, fixed at the canvas's left edge while its grid moves. */
  private readonly scrubber = document.createElement("div");
  /** The badge's text size on screen last written to `--fx-text`, within its fade. */
  private textSize = "";
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
    // One arrowhead per edge kind, so each head takes its line's colour.
    const markers = (["data", "control", "dependency"] as const).map((kind) => {
      const marker = element("marker", { id: `${this.id}-arrow-${kind}`, viewBox: "0 0 10 10", refX: 9, refY: 5, markerWidth: 6, markerHeight: 6, orient: "auto-start-reverse" });
      marker.append(element("path", { d: "M 0 0 L 10 5 L 0 10 z", class: `fx-canvas-arrow-${kind}` }));
      return marker;
    });
    const checker = element("pattern", { id: `${this.id}-checker`, patternUnits: "userSpaceOnUse", width: 16, height: 16 });
    checker.append(element("rect", { width: 16, height: 16, fill: "#fafafa" }));
    checker.append(element("path", { d: "M0 0h8v8H0z M8 8h8v8H8z", fill: "#ededf0" }));
    const shimmer = element("linearGradient", { id: `${this.id}-shimmer` });
    shimmer.append(
      element("stop", { offset: 0, "stop-color": "#fff", "stop-opacity": 0 }),
      element("stop", { offset: 0.5, "stop-color": "#fff", "stop-opacity": 0.85 }),
      element("stop", { offset: 1, "stop-color": "#fff", "stop-opacity": 0 }),
    );
    defs.append(...markers, checker, shimmer);
    this.content.style.transformOrigin = "0 0";
    this.svg.append(defs, this.content);
    this.toolbar.className = "fx-canvas-toolbar";
    const button = (label: string, text: string, action: () => void) => {
      const control = document.createElement("button");
      control.type = "button";
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
    this.scrubber.className = "fx-deck-scrubber";
    this.scrubber.hidden = true;
    // A row glides the camera to its card and outlines it; a click selects it. Leaving, the mark
    // goes back to the selected or resting member; the camera stays.
    const scrubbed = (event: Event) => {
      const row = (event.target as Element).closest<HTMLElement>(".fx-deck-scrubber-row");
      const index = this.open ? this.decks.indexOf(this.open.deck) : -1;
      return row && index >= 0 ? { index, at: Number(row.dataset.deckCard) } : null;
    };
    this.scrubber.addEventListener("pointerover", (event) => {
      const hit = scrubbed(event);
      if (!hit) return;
      this.mark(this.decks[hit.index].cards[hit.at]);
      this.markFront(this.decks[hit.index], hit.at);
      this.glideTo(hit.index, hit.at);
    }, { signal: this.abort.signal });
    this.scrubber.addEventListener("click", (event) => { const hit = scrubbed(event); if (hit) this.pick(hit.index, hit.at); }, { signal: this.abort.signal });
    this.scrubber.addEventListener("pointerleave", () => {
      this.mark(null);
      const index = this.open ? this.decks.indexOf(this.open.deck) : -1;
      if (index >= 0) this.markFront(this.decks[index], this.frontOf(index));
    }, { signal: this.abort.signal });
    container.replaceChildren(this.svg, this.toolbar, this.empty, this.deckBar, this.scrubber);
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
    this.shifts.clear();
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
      this.edges.set(edge.id, { group, source: edge.source, target: edge.target, points: edge.points });
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
    // Running and waiting cards count up together, once a second, until the next redraw.
    const clocks = [...this.content.querySelectorAll<SVGTextElement>("text[data-started]")];
    if (clocks.length) {
      const timer = window.setInterval(() => {
        const now = Date.now();
        for (const clock of clocks) clock.textContent = (clock.dataset.prefix ?? "") + runningLabel(Number(clock.dataset.started), now);
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
    const kind = edge.kind ?? "dependency";
    const path = element("path", { d, class: `fx-canvas-edge fx-canvas-edge-${kind}`, "data-edge-id": edge.id, "data-source-port": edge.source_port ?? "", "data-target-port": edge.target_port ?? "", "marker-end": `url(#${this.id}-arrow-${kind})`, role: "img", "aria-label": description });
    const hit = element("path", { d, class: "fx-canvas-edge-hit", "aria-hidden": "true" });
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
    // SVG does not size a rect to its text: the label is centred, so an estimate of its width
    // leaves even padding on both sides.
    const width = 20 + Math.ceil(6.2 * label.length);
    group.append(element("rect", { x: placed.x, y: placed.y, width, height: 22, rx: 11 }));
    group.append(element("text", { x: placed.x + width / 2, y: placed.y + 15, "text-anchor": "middle" }, label));
    group.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") { event.preventDefault(); this.expand(index); }
    }, { signal: this.graphAbort.signal });
    parent.append(group);
    // A mark is named as its card is titled (by its key where the titles are one, as a frame's
    // are), with its take where the deck holds several.
    const titles = members.map((member) => items.get(member.id)?.title ?? member.id);
    const distinct = new Set(titles).size === titles.length;
    const takes = members.some((member) => (member.take ?? 1) > 1);
    const names = members.map((member, at) => `${distinct ? titles[at] : member.key ?? titles[at]}${takes && member.take !== null ? ` · take ${member.take}` : ""}`);
    const bookmark = this.bookmark(placed, index, members, names);
    parent.append(bookmark);
    return { placed, title, parent, cards, boxes, members, badge: group, bookmark, names, start: 0 };
  }

  /**
   * §5.3: a deck's bookmark, right-aligned against its left edge: a mark per member, or per even
   * slice of members, and a row per member to hover (raise) and click (select). It lives with the
   * badge, so a nested deck's shows only while its card is in front.
   */
  private bookmark(placed: PlacedDeck, index: number, members: DeckMember[], names: string[]) {
    const group = element("g", { class: "fx-deck-bookmark", "aria-hidden": "true" });
    const ticks = deckTicks(members), right = placed.x - BOOKMARK_GAP, step = Math.min(BOOKMARK_STEP, BOOKMARK_BAND / ticks.length);
    // The first mark sits beside the front card's title.
    const top = placed.y + BADGE_BAND + 14 - step / 2, shape = bookmarkLayout(members.length, ticks.length, step, BOOKMARK_REACH);
    ticks.forEach((tick, at) => group.append(element("rect", { x: right - BOOKMARK_SHORT, y: top + shape.marks[at] - 1, width: BOOKMARK_SHORT, height: 2, rx: 1, class: `fx-deck-tick is-${tick.tone}`, "data-first": tick.first, "data-last": tick.last })));
    // Every row reaches the deck's edge.
    shape.rows.forEach((row, at) => {
      const hit = element("rect", { x: right - BOOKMARK_LONG - BOOKMARK_REACH, y: top + row.top, width: BOOKMARK_LONG + BOOKMARK_REACH + BOOKMARK_GAP, height: row.height, class: "fx-deck-bookmark-row", "data-deck": index, "data-deck-card": at });
      hit.append(element("title", {}, names[at]));
      hit.addEventListener("click", () => this.pick(index, at), { signal: this.graphAbort.signal });
      group.append(hit);
    });
    return group;
  }

  /** The bookmark's front mark: long and dark, the rest short. */
  private markFront(deck: Deck, front: number) {
    const holds = (mark: HTMLElement | SVGElement) => tickHolds({ first: Number(mark.dataset.first), last: Number(mark.dataset.last) }, front);
    // The open deck's marks are the scrubber's; its own bookmark is under the veil until it closes.
    if (this.open?.deck === deck) {
      for (const mark of this.scrubber.querySelectorAll<HTMLElement>(".fx-deck-tick")) mark.classList.toggle("is-front", holds(mark));
      return;
    }
    const right = deck.placed.x - BOOKMARK_GAP;
    for (const tick of deck.bookmark.querySelectorAll<SVGRectElement>(".fx-deck-tick")) {
      const width = holds(tick) ? BOOKMARK_LONG : BOOKMARK_SHORT;
      tick.classList.toggle("is-front", width === BOOKMARK_LONG);
      tick.setAttribute("x", String(right - width));
      tick.setAttribute("width", String(width));
    }
  }
  /**
   * A bookmark click: select the member, or the first step of a frame member, which then rests in
   * front; in the open deck the camera also glides to its card in the grid.
   */
  private pick(index: number, at: number) {
    const card = this.decks[index]?.cards[at];
    // A frame member's first step in layout order: the DOM order changes as its decks rest.
    const step = card && this.layout.nodes.find((node) => card.contains(this.nodes.get(node.id) ?? null));
    if (step) this.onSelect(step.id);
    this.glideTo(index, at);
  }
  /** In the open deck, the camera glides to a member's card in the grid. */
  private glideTo(index: number, at: number) {
    const grid = this.open?.deck === this.decks[index] ? this.open.grid : null;
    if (!grid) return;
    const box = { ...cell(grid, at), width: grid.width, height: grid.height };
    const bounds = this.container.getBoundingClientRect();
    this.animateCamera(() => this.viewport.reveal(box, bounds, { left: 0, right: 0, top: 0, bottom: 0 }, LEGIBLE_ZOOM));
  }
  /** §5.3: the open deck's bookmark, fixed at the canvas's left edge, from the same marks. */
  private showScrubber(index: number) {
    const deck = this.decks[index], ticks = deckTicks(deck.members);
    const shape = bookmarkLayout(deck.members.length, ticks.length, Math.min(SCRUBBER_STEP, SCRUBBER_BAND / ticks.length), BOOKMARK_REACH);
    this.scrubber.replaceChildren(...ticks.map((tick, at) => {
      const mark = document.createElement("span");
      mark.className = `fx-deck-tick is-${tick.tone}`;
      Object.assign(mark.dataset, { first: String(tick.first), last: String(tick.last) });
      mark.style.top = `${shape.marks[at] - 1}px`;
      return mark;
    }), ...shape.rows.map((row, at) => {
      const hit = document.createElement("button");
      hit.type = "button";
      hit.className = "fx-deck-scrubber-row";
      hit.dataset.deckCard = String(at);
      hit.title = deck.names[at];
      hit.setAttribute("aria-label", deck.names[at]);
      Object.assign(hit.style, { top: `${row.top}px`, height: `${row.height}px` });
      return hit;
    }));
    this.scrubber.style.height = `${shape.height}px`;
    this.scrubber.hidden = false;
    this.markFront(deck, this.frontOf(index));
  }
  /** Outlines the card a bookmark mark points at in the open deck. */
  private mark(card: SVGGElement | null) {
    if (this.marked === card) return;
    this.marked?.classList.remove("is-marked");
    this.marked = card;
    card?.classList.add("is-marked");
  }
  /** The deck card that is or holds this node's card, or -1. */
  private holding(deck: Deck, id: string) {
    const node = this.nodes.get(id);
    return node ? deck.cards.findIndex((card) => card.contains(node)) : -1;
  }

  /**
   * §5.3, §5.4: the closed stack shows the members around the front, the front last in its layer
   * and the badge above it. `keep` holds the stack where it is while the front stays in it.
   * Returns the front.
   */
  private restDeck(index: number, front = this.frontOf(index), keep = false) {
    const deck = this.decks[index];
    if (this.open?.deck === deck) return null;
    const stack = deckStack(deck.members.length, front, keep ? deck.start : undefined);
    deck.start = stack.start;
    // Only the shown cards' order shows; the badge and the bookmark stay above them.
    for (const at of stack.order) if (stack.shown[at]) deck.parent.insertBefore(deck.cards[at], deck.badge);
    this.stack(deck, stack.slots);
    deck.cards.forEach((card, at) => card.classList.toggle("is-buried", !stack.shown[at]));
    this.markFront(deck, front);
    // One tab stop per closed deck: its front card.
    deck.cards.forEach((card, at) => { if (card.hasAttribute("tabindex")) card.setAttribute("tabindex", at === front ? "0" : "-1"); });
    return deck.cards[front];
  }
  /**
   * Moves each card from where the layout put it (by position) to its place in the stack, and the
   * ends of its wires with it.
   */
  private stack(deck: Deck, slots: number[]) {
    const rewire = new Set<string>();
    deck.cards.forEach((card, at) => {
      const by = (slots[at] - Math.min(at, DECK_BACK)) * DECK_OFFSET, box = deck.boxes[at];
      place(card, box, by === 0 ? null : { x: box.x + by, y: box.y + by });
      if (by === (this.shifts.get(card) ?? 0)) return;
      this.shifts.set(card, by);
      for (const node of card.matches("[data-node-id]") ? [card] : card.querySelectorAll<SVGGElement>("[data-node-id]")) {
        for (const id of this.incidentEdges.get(node.dataset.nodeId!) ?? []) rewire.add(id);
      }
    });
    for (const id of rewire) this.rewire(id);
  }
  /** Redraws a wire between its cards where the stacks have moved them. */
  private rewire(id: string) {
    const edge = this.edges.get(id)!;
    // The wire's layer moves with its own frame; only the cards inside it move its ends.
    const layer = edge.group.parentElement!.parentElement;
    const shift = (node: string) => {
      let by = 0;
      for (let at = this.nodes.get(node) as Element | null; at && at !== layer; at = at.parentElement) by += this.shifts.get(at as SVGGElement) ?? 0;
      return by;
    };
    const from = shift(edge.source), to = shift(edge.target), last = edge.points.length - 1;
    // An end and the point beside it, where a backward wire turns, move with the card.
    const points = edge.points.map((point, at) => {
      const by = at === 0 || (at === 1 && last > 1) ? from : at === last || (at === last - 1 && last > 1) ? to : 0;
      return { x: point.x + by, y: point.y + by };
    });
    const d = canvasEdgePath(points);
    for (const path of edge.group.querySelectorAll("path")) path.setAttribute("d", d);
  }
  /** The member that rests in front of a closed deck (§5.4). */
  private frontOf(index: number) {
    return restingFront(this.decks[index].members, this.selectedMember(index));
  }
  private selectedMember(index: number) {
    if (this.selected === null) return null;
    const deck = this.decks[index];
    const at = this.holding(deck, this.selected);
    return at >= 0 ? deck.members[at].id : null;
  }

  private cardAt(target: EventTarget | null): DeckHit | null {
    const card = (target as Element | null)?.closest?.<SVGElement>(".fx-deck-card");
    if (!card || card.dataset.deck === undefined) return null;
    return { deck: Number(card.dataset.deck), card: Number(card.dataset.deckCard) };
  }
  /** A deck card under the pointer, or a bookmark row pointing at one. */
  private hoverAt(target: EventTarget | null): DeckHit | null {
    const row = (target as Element | null)?.closest?.<SVGElement>(".fx-deck-bookmark-row");
    return row ? { deck: Number(row.dataset.deck), card: Number(row.dataset.deckCard), mark: true } : this.cardAt(target);
  }
  /**
   * Hover raises the card under the pointer, in place; a bookmark mark brings its member's
   * neighbours into the stack too. Leaving restores the resting card.
   */
  private hover(target: DeckHit | null) {
    if (this.open || !this.canHover.matches) target = null;
    const previous = this.hovered;
    if (previous?.deck === target?.deck && previous?.card === target?.card) return;
    this.hovered = target;
    if (previous && previous.deck !== target?.deck) this.restDeck(previous.deck);
    if (target) this.restDeck(target.deck, target.card, !target.mark);
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
    const grid = { ...origin, columns, ...card };
    // Moved cards start their transition from where they rest.
    this.content.getBoundingClientRect();
    deck.cards.forEach((element, at) => {
      if (element.hasAttribute("tabindex")) element.setAttribute("tabindex", "0");
      element.classList.remove("is-buried");
      element.style.transition = `transform ${duration}ms ease`;
      place(element, sizes[at], cell(grid, at));
    });
    veil.style.transition = `opacity ${duration}ms ease`;
    veil.classList.add("is-shown");
    this.open = { deck, veil, camera, grid };
    this.showScrubber(index);
    this.deckBar.replaceChildren();
    const heading = document.createElement("span");
    heading.className = "fx-deck-bar-title";
    heading.textContent = `${deck.title} · ${deck.members.length}`;
    const hint = document.createElement("span");
    hint.className = "fx-deck-bar-hint";
    // Kept short: a click outside the deck (the veil) closes it too, as does ×.
    hint.textContent = "Esc to close";
    const close = document.createElement("button");
    close.type = "button";
    close.append(createElement(X, { width: 14, height: 14, "stroke-width": 2, "aria-hidden": "true" }));
    close.setAttribute("aria-label", "Close");
    close.addEventListener("click", () => this.collapse(), { signal: this.graphAbort.signal });
    this.deckBar.append(heading, hint, close);
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
    this.mark(null);
    this.deckBar.hidden = true;
    this.scrubber.hidden = true;
    this.scrubber.replaceChildren();
    const duration = instant ? 0 : this.motion;
    for (const card of open.deck.cards) card.style.transition = `transform ${duration}ms ease`;
    // They slide straight to their places in the stack; settling restacks them as it raises the front.
    const at = this.decks.indexOf(open.deck);
    if (at >= 0) this.stack(open.deck, deckStack(open.deck.members.length, this.frontOf(at)).slots);
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
    const held = this.open && id !== null ? this.holding(this.open.deck, id) : -1;
    if (this.open && id !== null && held < 0) this.collapse(true);
    // In the open deck the bookmark marks the selected member.
    if (this.open && held >= 0) this.markFront(this.open.deck, held);
    this.decks.forEach((deck, index) => {
      if ([previous, id].some((key) => key !== null && this.holding(deck, key) >= 0)) this.restDeck(index);
    });
  }
  /**
   * A card chosen outside the canvas, from a list: the camera glides to it, in the area the
   * caller's panels leave free. A member of the open deck is already in view in its grid.
   */
  reveal(id: string, inset: CanvasInset = { left: 0, right: 0, top: 0, bottom: 0 }) {
    const node = this.layout.nodes.find((item) => item.id === id);
    if (!node || (this.open && this.holding(this.open.deck, id) >= 0)) return;
    const bounds = this.container.getBoundingClientRect();
    this.animateCamera(() => this.viewport.reveal(node, bounds, inset, LEGIBLE_ZOOM));
  }
  private highlightConnections(nodeId: string | null, edgeId: string | null = null) {
    const next = this.drag?.moved ? new Set<string>() : nodeId !== null
      ? this.incidentEdges.get(nodeId) ?? new Set<string>()
      : new Set(edgeId === null ? [] : [edgeId]);
    for (const id of this.highlightedEdges) if (!next.has(id)) this.edges.get(id)?.group.classList.remove("is-highlighted");
    for (const id of next) if (!this.highlightedEdges.has(id)) this.edges.get(id)?.group.classList.add("is-highlighted");
    this.highlightedEdges = next;
  }
  /** The camera in screen pixels, for a page that moves it itself; input still moves it too. */
  getCamera(): ViewportState { return this.viewport.getSnapshot(); }
  /** Sets the camera at once, like input does; a set camera counts as framed, so a resize keeps it. */
  setCamera(camera: ViewportState) {
    this.content.style.transition = "";
    this.viewport.restore(camera);
    this.didFit = true;
    this.applyViewport();
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
    // §5.3: a badge too small to read fades out with the zoom (--fx-text); a bookmark smaller
    // still hides too.
    // Clamped to the fade, so zooming outside it restyles nothing.
    const text = String(Math.min(9, Math.max(8, zoom * 13)));
    if (text !== this.textSize) this.content.style.setProperty("--fx-text", this.textSize = text);
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
      background: !target.closest("[data-node-id], [data-connection-id], [data-deck], .fx-deck-bookmark"), veil: Boolean(target.closest(".fx-deck-veil")) };
  };
  private onPointerMove = (event: PointerEvent) => {
    if (!this.drag) { this.hover(this.hoverAt(event.target)); return; }
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
