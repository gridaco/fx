import { valuePreviewLines, valueStateLabel } from "./value-views";
import type { CanvasNote } from "./notes";
import type { CanvasConnection, CanvasFrame, CanvasGraph, CanvasStep, LayoutReport } from "./graph";
import { DECK_BACK } from "./deck";

export interface PositionedNode extends CanvasStep { x: number; y: number; width: number; height: number }
export interface PositionedNote extends CanvasNote { x: number; y: number; width: number; height: number }
export const NOTE_WIDTH = 360, NOTE_MIN_HEIGHT = 96, NOTE_MAX_HEIGHT = 720;
export interface PositionedEdge extends CanvasConnection { points: { x: number; y: number }[] }
export interface PositionedFrame extends CanvasFrame { x: number; y: number; width: number; height: number }
/** A workflow interface's presentation container; children retain their own value identities. */
export interface PositionedBoundaryGroup { side: "input" | "output"; title: string; nodes: string[]; x: number; y: number; width: number; height: number }
/** Two or more members of one slot in one frame: node ids, or frame ids for repeated groups. */
export interface PlacedDeck { id: string; address: string; frame: string | null; kind: "node" | "frame"; members: string[]; x: number; y: number; width: number; height: number }
export interface CanvasLayout { nodes: PositionedNode[]; edges: PositionedEdge[]; frames: PositionedFrame[]; decks: PlacedDeck[]; notes?: PositionedNote[]; boundary_groups?: PositionedBoundaryGroup[]; width: number; height: number }

export const DECK_OFFSET = 28, BADGE_BAND = 30;
const COLUMN_GAP = 100, ROW_GAP = 42, FRAME_PAD = 20, FRAME_TOP = 36, MARGIN = 20;
const BOUNDARY_PAD = 20, BOUNDARY_TOP = 40, BOUNDARY_GAP = 16;
/** Socket rows: the card draws them and edges attach to them. */
export const PORT_TOP = 93, PORT_ROW = 34;

/** §4: declared visual media reserves its preview area before any file exists or is decoded. */
export function canvasPreviewReserved(node: CanvasStep) {
  return node.preview !== undefined || node.value_media === true || (node.ports?.outputs.some((port) => port.kind === "artifact" && /^(?:image|video|model)(?:[/[{?]|$)/.test(port.type)) ?? false);
}

/** A preview frame's height for its width: 16:9, the commonest shape of what steps make. */
const PREVIEW_ASPECT = 9 / 16;

/** A waiting card's footer occupies its own band below ports, settings and any preview. */
export function reviewFooter(node: CanvasStep) {
  const pending = node.review_action === "review" || (node.state === "awaiting" && !node.review_action);
  if (node.kind || (!pending && !node.review_action)) return undefined;
  return { pending, height: pending ? 66 : 48 };
}

/**
 * Geometry depends on declarations, never on media loading or measured DOM text. The preview
 * frame spans the card less 16 on each side.
 */
export function canvasNodeGeometry(node: CanvasStep) {
  const review = reviewFooter(node);
  const width = node.ports || review ? 360 : 250;
  if (node.value) {
    const lines = valuePreviewLines(valueStateLabel(node.value)).lines.length;
    const preview_y = 80 + Math.max(1, lines) * 18;
    const preview_height = Math.round((width - 32) * PREVIEW_ASPECT);
    return { width, height: preview_y + (canvasPreviewReserved(node) ? preview_height + 14 : 0) + 24, preview_y, preview_height, settings_y: 60 };
  }
  const preview_height = Math.round((width - 32) * PREVIEW_ASPECT);
  // The media count sits above a review footer, clear of the message and action.
  const footer = review ? review.height + (node.preview ? 20 : 0) : undefined;
  if (!node.ports) return { width, height: node.preview ? 61 + preview_height + (footer ?? 45) : 75 + (footer ?? 31), preview_y: 61, preview_height, settings_y: 0 };
  const rows = Math.max(node.ports.inputs.length, node.ports.outputs.length);
  const settings_y = 83 + Math.max(1, rows) * PORT_ROW;
  const summaryRows = node.summaries?.length ?? 0;
  const summaryHeight = summaryRows ? 20 + summaryRows * 34 + ((node.summary_count ?? 0) > summaryRows ? 18 : 0) : node.ports.settings.length ? 29 : 5;
  const preview_y = settings_y + summaryHeight + (node.route_label ? 20 : 0);
  return { width, height: preview_y + (canvasPreviewReserved(node) ? preview_height + 16 : 0) + (footer ?? (node.kind === "workflow" ? 70 : 42)), preview_y, preview_height, settings_y };
}
export function canvasNodeSize(node: CanvasStep) {
  const { width, height } = canvasNodeGeometry(node);
  return { width, height };
}

/** A named socket's position; unknown names have no invented attachment point. */
export function canvasPortAnchor(node: PositionedNode, direction: "input" | "output", name: string, kind?: "artifact" | "parameter" | "fact" | "value") {
  const ports = direction === "input" ? node.ports?.inputs : node.ports?.outputs;
  if (node.value && ports?.some((port) => port.name === name)) return { x: node.x + (direction === "input" ? 0 : node.width), y: node.y + 28 };
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
interface Item { id: string; address: string; rank: number; node?: CanvasStep; frame?: CanvasFrame; note?: CanvasNote }
interface Slot { address: string; items: Item[]; column: number; row: number }

/**
 * Places the projection in the served cells (spec/layout.md §4 rule 11, §5.3). The browser never
 * computes a cell: without a report, or for an address the report does not place yet, slots go in
 * one row in projection order until the next report arrives. Boundary cards of an opened imported
 * workflow sit before the first and after the last column.
 */
export function layoutGraph(graph: CanvasGraph, report: LayoutReport | null = null,
  noteSizes: ReadonlyMap<string, { width: number; height: number }> = new Map()): CanvasLayout {
  const frames = graph.frames ?? [];
  const frameById = new Map(frames.map((frame) => [frame.id, frame]));
  const parentOf = new Map<string, string>();
  for (const frame of frames) for (const id of frame.nodes) parentOf.set(id, frame.id);
  const rank = new Map((report?.order ?? []).map((id, index) => [id, index]));
  const item = (id: string, source: string, address: string | undefined, card: Pick<Item, "node" | "frame" | "note">): Item =>
    ({ id, address: address ?? id, rank: rank.get(source) ?? rank.size + 1, ...card });
  const containerAddress = (frame: string | null) => frame === null ? "" : frameById.get(frame)?.address ?? frame;

  // Every container instance (the root, every frame) and its slots.
  const contents = new Map<string | null, Item[]>([[null, []]]);
  for (const frame of frames) contents.set(frame.id, []);
  for (const node of graph.nodes) contents.get(parentOf.get(node.id) ?? null)!.push(item(node.id, node.source_id, node.address, { node }));
  for (const frame of frames) contents.get(frame.parent && frameById.has(frame.parent) ? frame.parent : null)!.push(item(frame.id, frame.id, frame.address, { frame }));
  for (const note of graph.notes ?? []) contents.get(note.frame_id && contents.has(note.frame_id) ? note.frame_id : null)!.push(item(note.id, note.id, note.id, { note }));
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
      const value = members[0].node?.value;
      const cell = members[0].note ? report?.notes?.[address] : value ? report?.values?.[members[0].id] : report?.cells[address];
      const column = value ? value.kind === "output" ? Number.MAX_SAFE_INTEGER : value.kind === "input" ? -2 : -1
        : side === "input" ? -1 : side === "output" ? Number.MAX_SAFE_INTEGER : cell?.column ?? fallback++;
      const valueRow = value ? cell?.row ?? placed.filter((slot) => slot.column === column).length : undefined;
      placed.push({ address, items: members, column, row: valueRow ?? (side ? 0 : cell?.row ?? 0) });
    }
    slots.set(container, placed);
  }

  // A workflow's public values form compact side panels, independent of processing rows.
  // An opened import is already projected at the canvas root; inlined group values keep
  // their frame ownership and ordinary value placement.
  const rootSlots = slots.get(null)!;
  const boundarySlots = (["input", "output"] as const).map((side) => ({ side,
    slots: rootSlots.filter((slot) => slot.items[0].node?.value?.kind === side)
      .sort((left, right) => left.row - right.row || (left.address < right.address ? -1 : left.address > right.address ? 1 : 0)),
  })).filter((group) => group.slots.length > 0);
  const boundaryMembers = new Set(boundarySlots.flatMap((group) => group.slots));
  slots.set(null, rootSlots.filter((slot) => !boundaryMembers.has(slot)));

  // Rule 11: tracks span every frame of a container, so a group's frames are the same size.
  const tracks = new Map<string, { columns: Map<number, number>; rows: Map<number, number> }>();
  const frameSize = new Map<string, { width: number; height: number }>();
  // Captured note text is measured before placement; the grid still owns its cells and tracks.
  const noteSize = (id: string) => noteSizes.get(id) ?? { width: NOTE_WIDTH, height: NOTE_MIN_HEIGHT };
  const itemSize = (each: Item) => each.note ? noteSize(each.note.id) : each.node ? canvasNodeSize(each.node) : sizeOfFrame(each.frame!);
  const footprint = (slot: Slot) => {
    const sizes = slot.items.map(itemSize);
    return deckFootprint({ width: Math.max(...sizes.map((size) => size.width)), height: Math.max(...sizes.map((size) => size.height)) }, slot.items.length);
  };
  const boundarySizes = boundarySlots.map((group) => {
    const sizes = group.slots.map(footprint);
    return { ...group, column: group.side === "input" ? -2 : Number.MAX_SAFE_INTEGER,
      width: Math.max(...sizes.map((size) => size.width)) + BOUNDARY_PAD * 2,
      height: BOUNDARY_TOP + sizes.reduce((height, size) => height + size.height, 0) + BOUNDARY_GAP * (sizes.length - 1) + BOUNDARY_PAD };
  });
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
    if (address === containerAddress(null)) for (const group of boundarySizes) {
      found.columns.set(group.column, Math.max(found.columns.get(group.column) ?? 0, group.width));
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

  const notes: PositionedNote[] = [];
  const nodes: PositionedNode[] = [];
  const positionedFrames: PositionedFrame[] = [];
  const decks: PlacedDeck[] = [];
  const placeSlot = (slot: Slot, container: string | null, x: number, y: number) => {
    const deck = slot.items.length > 1;
    if (deck) {
      const size = footprint(slot);
      decks.push({ id: `${container ?? ""}|${slot.address}`, address: slot.address, frame: container, kind: slot.items[0].node ? "node" : "frame",
        members: slot.items.map((each) => each.id), x, y, ...size });
    }
    slot.items.forEach((each, index) => {
      const offset = DECK_OFFSET * Math.min(index, DECK_BACK);
      const at = { x: x + offset, y: y + offset + (deck ? BADGE_BAND : 0) };
      if (each.note) notes.push({ ...each.note, ...at, ...noteSize(each.note.id) });
      else if (each.node) nodes.push({ ...each.node, ...at, ...canvasNodeSize(each.node) });
      else {
        positionedFrames.push({ ...each.frame!, ...at, ...sizeOfFrame(each.frame!) });
        place(each.id, at.x + FRAME_PAD, at.y + FRAME_TOP);
      }
    });
  };
  const place = (container: string | null, originX: number, originY: number) => {
    const { columns, rows } = tracksOf(containerAddress(container));
    const xs = offsets(columns, COLUMN_GAP), ys = offsets(rows, ROW_GAP);
    for (const slot of slots.get(container) ?? []) {
      const x = originX + xs.get(slot.column)!, y = originY + ys.get(slot.row)!;
      placeSlot(slot, container, x, y);
    }
  };
  place(null, MARGIN, MARGIN);
  const rootColumns = offsets(tracksOf(containerAddress(null)).columns, COLUMN_GAP);
  const boundaryGroups = boundarySizes.map((group): PositionedBoundaryGroup => {
    const x = MARGIN + rootColumns.get(group.column)!, y = MARGIN;
    let nextY = y + BOUNDARY_TOP;
    for (const slot of group.slots) {
      placeSlot(slot, null, x + BOUNDARY_PAD, nextY);
      nextY += footprint(slot).height + BOUNDARY_GAP;
    }
    return { side: group.side, title: group.side === "input" ? "Inputs" : "Outputs", nodes: group.slots.flatMap((slot) => slot.items.map((item) => item.id)),
      x, y, width: group.width, height: group.height };
  });
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
  const right = Math.max(0, ...notes.map((note) => note.x + note.width), ...nodes.map((node) => node.x + node.width), ...positionedFrames.map((frame) => frame.x + frame.width), ...boundaryGroups.map((group) => group.x + group.width));
  const bottom = Math.max(0, ...notes.map((note) => note.y + note.height), ...nodes.map((node) => node.y + node.height), ...positionedFrames.map((frame) => frame.y + frame.height), ...boundaryGroups.map((group) => group.y + group.height));
  return { nodes, notes, edges, frames: positionedFrames, decks, ...(boundaryGroups.length ? { boundary_groups: boundaryGroups } : {}), width: right + MARGIN, height: bottom + MARGIN };
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
