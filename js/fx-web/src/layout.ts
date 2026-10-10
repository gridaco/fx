import type { CanvasConnection, CanvasFrame, CanvasGraph, CanvasStep, LayoutReport } from "./graph";

export interface PositionedNode extends CanvasStep { x: number; y: number; width: number; height: number }
export interface PositionedEdge extends CanvasConnection { points: { x: number; y: number }[] }
export interface PositionedFrame extends CanvasFrame { x: number; y: number; width: number; height: number }
/** Two or more members of one slot in one frame: node ids, or frame ids for repeated groups. */
export interface PlacedDeck { id: string; address: string; frame: string | null; kind: "node" | "frame"; members: string[]; x: number; y: number; width: number; height: number }
export interface CanvasLayout { nodes: PositionedNode[]; edges: PositionedEdge[]; frames: PositionedFrame[]; decks: PlacedDeck[]; width: number; height: number }

export const DECK_OFFSET = 28, BADGE_BAND = 30;
const COLUMN_GAP = 100, ROW_GAP = 42, DECK_BACK = 3, FRAME_PAD = 20, FRAME_TOP = 36, MARGIN = 20;
/** Socket rows: the card draws them and edges attach to them. */
export const PORT_TOP = 93, PORT_ROW = 34;

/** §4: a card with a declared image output reserves its preview area before any image exists. */
export function canvasPreviewReserved(node: CanvasStep) {
  return node.preview !== undefined || (node.ports?.outputs.some((port) => port.kind === "artifact" && /^image(?:[/[{?]|$)/.test(port.type)) ?? false);
}

/** Geometry depends on declarations, never on image loading or measured DOM text. */
export function canvasNodeGeometry(node: CanvasStep) {
  if (!node.ports) return { width: 250, height: node.preview ? 246 : 106, preview_y: 61, settings_y: 0 };
  const rows = Math.max(node.ports.inputs.length, node.ports.outputs.length);
  const settings_y = 83 + Math.max(1, rows) * PORT_ROW;
  const preview_y = settings_y + (node.ports.settings.length ? 29 : 5);
  return { width: 360, height: preview_y + (canvasPreviewReserved(node) ? 140 + 16 : 0) + (node.kind === "workflow" ? 70 : 42), preview_y, settings_y };
}
export function canvasNodeSize(node: CanvasStep) {
  const { width, height } = canvasNodeGeometry(node);
  return { width, height };
}

/** A named socket's position; unknown names have no invented attachment point. */
export function canvasPortAnchor(node: PositionedNode, direction: "input" | "output", name: string, kind?: "artifact" | "parameter" | "fact" | "value") {
  const ports = direction === "input" ? node.ports?.inputs : node.ports?.outputs;
  const index = ports?.findIndex((port) => port.name === name && (!kind || port.kind === kind)) ?? -1;
  if (index < 0) return undefined;
  return { x: node.x + (direction === "input" ? 0 : node.width), y: node.y + PORT_TOP + index * PORT_ROW };
}

/** A deck's footprint: its largest member plus at most three offset cards and the badge band. */
function deckFootprint(member: { width: number; height: number }, count: number) {
  if (count < 2) return member;
  const offset = DECK_OFFSET * Math.min(count - 1, DECK_BACK);
  return { width: member.width + offset, height: member.height + offset + BADGE_BAND };
}

/** A card in its container: a node, or a frame with its own contents. */
interface Item { id: string; address: string; rank: number; node?: CanvasStep; frame?: CanvasFrame }
interface Slot { address: string; items: Item[]; column: number; row: number }

/**
 * Places the projection in the served cells (spec/layout.md §4 rule 11, §5.3). The browser never
 * computes a cell: without a report, or for an address the report does not place yet, slots go in
 * one row in projection order until the next report arrives. Boundary cards of an opened imported
 * workflow sit before the first and after the last column.
 */
export function layoutGraph(graph: CanvasGraph, report: LayoutReport | null = null): CanvasLayout {
  const frames = graph.frames ?? [];
  const frameById = new Map(frames.map((frame) => [frame.id, frame]));
  const parentOf = new Map<string, string>();
  for (const frame of frames) for (const id of frame.nodes) parentOf.set(id, frame.id);
  const rank = new Map((report?.order ?? []).map((id, index) => [id, index]));
  const item = (id: string, source: string, address: string | undefined, card: Pick<Item, "node" | "frame">): Item =>
    ({ id, address: address ?? id, rank: rank.get(source) ?? rank.size + 1, ...card });
  const containerAddress = (frame: string | null) => frame === null ? "" : frameById.get(frame)?.address ?? frame;

  // Every container instance (the root, every frame) and its slots.
  const contents = new Map<string | null, Item[]>([[null, []]]);
  for (const frame of frames) contents.set(frame.id, []);
  for (const node of graph.nodes) contents.get(parentOf.get(node.id) ?? null)!.push(item(node.id, node.source_id, node.address, { node }));
  for (const frame of frames) contents.get(frame.parent && frameById.has(frame.parent) ? frame.parent : null)!.push(item(frame.id, frame.id, frame.address, { frame }));
  const slots = new Map<string | null, Slot[]>();
  for (const [container, items] of contents) {
    const byAddress = new Map<string, Item[]>();
    for (const each of items) {
      if (!byAddress.has(each.address)) byAddress.set(each.address, []);
      byAddress.get(each.address)!.push(each);
    }
    const placed: Slot[] = [];
    let fallback = Math.max(-1, ...[...byAddress.keys()].map((address) => report?.cells[address]?.column ?? -1)) + 1;
    for (const [address, all] of byAddress) {
      // §5.1: absent instances are never members; a slot with only absent ones draws one card.
      const present = all.filter((each) => (each.node ?? each.frame)?.state !== "absent");
      const members = present.length ? present : all.slice(0, 1);
      members.sort((left, right) => left.rank - right.rank || (left.id < right.id ? -1 : 1));
      const side = members[0].node?.side;
      const cell = report?.cells[address];
      const column = side === "input" ? -1 : side === "output" ? Number.MAX_SAFE_INTEGER : cell?.column ?? fallback++;
      placed.push({ address, items: members, column, row: side ? 0 : cell?.row ?? 0 });
    }
    slots.set(container, placed);
  }

  // Rule 11: tracks span every frame of a container, so a group's frames are the same size.
  const tracks = new Map<string, { columns: Map<number, number>; rows: Map<number, number> }>();
  const frameSize = new Map<string, { width: number; height: number }>();
  const itemSize = (each: Item) => each.node ? canvasNodeSize(each.node) : sizeOfFrame(each.frame!);
  const footprint = (slot: Slot) => {
    const sizes = slot.items.map(itemSize);
    return deckFootprint({ width: Math.max(...sizes.map((size) => size.width)), height: Math.max(...sizes.map((size) => size.height)) }, slot.items.length);
  };
  function tracksOf(address: string) {
    let found = tracks.get(address);
    if (found) return found;
    found = { columns: new Map(), rows: new Map() };
    tracks.set(address, found);
    for (const [container, list] of slots) {
      if (containerAddress(container) !== address) continue;
      for (const slot of list) {
        const size = footprint(slot);
        found.columns.set(slot.column, Math.max(found.columns.get(slot.column) ?? 0, size.width));
        found.rows.set(slot.row, Math.max(found.rows.get(slot.row) ?? 0, size.height));
      }
    }
    return found;
  }
  const extent = (sizes: Map<number, number>, gap: number) => [...sizes.values()].reduce((sum, size) => sum + size, 0) + gap * Math.max(0, sizes.size - 1);
  function sizeOfFrame(frame: CanvasFrame) {
    let size = frameSize.get(frame.id);
    if (size) return size;
    const { columns, rows } = tracksOf(containerAddress(frame.id));
    size = { width: Math.max(220, extent(columns, COLUMN_GAP) + FRAME_PAD * 2), height: extent(rows, ROW_GAP) + FRAME_TOP + FRAME_PAD };
    frameSize.set(frame.id, size);
    return size;
  }
  const offsets = (sizes: Map<number, number>, gap: number) => {
    const start = new Map<number, number>();
    let at = 0;
    for (const index of [...sizes.keys()].sort((a, b) => a - b)) { start.set(index, at); at += sizes.get(index)! + gap; }
    return start;
  };

  const nodes: PositionedNode[] = [];
  const positionedFrames: PositionedFrame[] = [];
  const decks: PlacedDeck[] = [];
  const place = (container: string | null, originX: number, originY: number) => {
    const { columns, rows } = tracksOf(containerAddress(container));
    const xs = offsets(columns, COLUMN_GAP), ys = offsets(rows, ROW_GAP);
    for (const slot of slots.get(container) ?? []) {
      const x = originX + xs.get(slot.column)!, y = originY + ys.get(slot.row)!;
      const deck = slot.items.length > 1;
      if (deck) {
        const size = footprint(slot);
        decks.push({ id: `${container ?? ""}|${slot.address}`, address: slot.address, frame: container, kind: slot.items[0].node ? "node" : "frame",
          members: slot.items.map((each) => each.id), x, y, ...size });
      }
      slot.items.forEach((each, index) => {
        const offset = DECK_OFFSET * Math.min(index, DECK_BACK);
        const at = { x: x + offset, y: y + offset + (deck ? BADGE_BAND : 0) };
        if (each.node) nodes.push({ ...each.node, ...at, ...canvasNodeSize(each.node) });
        else {
          positionedFrames.push({ ...each.frame!, ...at, ...sizeOfFrame(each.frame!) });
          place(each.id, at.x + FRAME_PAD, at.y + FRAME_TOP);
        }
      });
    }
  };
  place(null, MARGIN, MARGIN);
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const frameIds = new Set(positionedFrames.map((frame) => frame.id));
  for (const frame of positionedFrames) frame.nodes = frame.nodes.filter((id) => byId.has(id) || frameIds.has(id));
  const edges = graph.edges.flatMap((edge) => {
    const source = byId.get(edge.source), target = byId.get(edge.target);
    if (!source || !target) return [];
    const from = edge.kind === "data" && edge.source_port !== undefined
      ? canvasPortAnchor(source, "output", edge.source_port, edge.source_kind === "fact" ? "fact" : undefined)
      : { x: source.x + source.width, y: source.y + 27 };
    const to = edge.kind === "data" && edge.target_port
      ? canvasPortAnchor(target, "input", edge.target_port)
      : { x: target.x, y: target.y + 27 };
    if (!from || !to) throw new Error(`Connection ${edge.id} refers to a missing named port.`);
    return [{ ...edge, points: route(from, to, source, target) }];
  });
  const right = Math.max(0, ...nodes.map((node) => node.x + node.width), ...positionedFrames.map((frame) => frame.x + frame.width));
  const bottom = Math.max(0, ...nodes.map((node) => node.y + node.height), ...positionedFrames.map((frame) => frame.y + frame.height));
  return { nodes, edges, frames: positionedFrames, decks, width: right + MARGIN, height: bottom + MARGIN };
}

/** §9: a forward edge is a curve between sockets; a backward one runs through the gap below both cards. */
function route(from: { x: number; y: number }, to: { x: number; y: number }, source: PositionedNode, target: PositionedNode) {
  if (to.x > from.x + 32) return [from, to];
  const below = Math.max(source.y + source.height, target.y + target.height) + ROW_GAP / 2;
  return [from, { x: from.x + 24, y: from.y }, { x: from.x + 24, y: below }, { x: to.x - 24, y: below }, { x: to.x - 24, y: to.y }, to];
}

/** A forward edge uses horizontal tangents; a backward one is a rounded polyline. */
export function canvasEdgePath(points: PositionedEdge["points"]): string {
  const route = points.filter((point, index) => !index || point.x !== points[index - 1].x || point.y !== points[index - 1].y);
  if (!route.length) return "";
  const from = route[0], to = route.at(-1)!;
  const start = `M ${from.x} ${from.y}`;
  if (route.length === 2 && to.x > from.x) {
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
