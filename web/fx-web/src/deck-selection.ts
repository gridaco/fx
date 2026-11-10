import type { CanvasLayout } from "./layout";
import { restingRow, type DeckMember } from "./deck";

export interface DeckPosition { deck: number; card: number }
interface LinkedDecks { decks: number[]; rows: number[][] }

/**
 * Read-only collapsed-deck representatives. Topology locates members and explicit judge
 * partners; only consumption evidence participates in the chosen-result priority.
 */
export class DeckSelection {
  readonly members: DeckMember[][];
  private readonly owners = new Map<string, Map<number, number>>();
  private readonly groups: { linked: LinkedDecks; column: number }[] = [];

  constructor(layout: CanvasLayout) {
    const items = new Map([...layout.nodes, ...layout.frames].map((item) => [item.id, item]));
    const children = new Map(layout.frames.map((frame) => [frame.id, [...frame.nodes, ...layout.frames.filter((child) => child.parent === frame.id).map((child) => child.id)]]));
    this.members = layout.decks.map((deck, index) => deck.members.map((id, at) => {
      const item = items.get(id)!;
      const pending = [id], seen = new Set<string>();
      while (pending.length) {
        const descendant = pending.pop()!;
        if (seen.has(descendant)) continue;
        seen.add(descendant);
        const owners = this.owners.get(descendant) ?? new Map<number, number>();
        owners.set(index, at);
        this.owners.set(descendant, owners);
        pending.push(...children.get(descendant) ?? []);
      }
      return { id, state: item.state ?? "planned", key: item.key ?? null, take: item.take?.at(-1) ?? null,
        ...(item.executed ? { executed: true as const } : {}) };
    }));
    for (const edge of layout.edges) {
      if (!edge.consumed) continue;
      for (const [deck, at] of this.owners.get(edge.source) ?? []) {
        if (!this.owners.get(edge.target)?.has(deck)) this.members[deck][at].bound = true;
      }
    }

    // Link only complete explicit judge correspondences in the same frame. Matching take
    // numbers alone would link unrelated work; a partial or ambiguous mapping stays separate.
    const nodeById = new Map(layout.nodes.map((node) => [node.id, node]));
    const direct = new Map(layout.decks.flatMap((deck, index) => deck.kind === "node" && deck.members.every((id) => !nodeById.get(id)?.kind)
      ? deck.members.map((id, at) => [id, { deck: index, card: at }] as const) : []));
    const candidates = new Map<string, { left: number; right: number; pairs: Map<number, Set<number>> }>();
    for (const edge of layout.edges) {
      if (!edge.kinds.includes("judges")) continue;
      const from = direct.get(edge.source), to = direct.get(edge.target);
      if (!from || !to || from.deck === to.deck || layout.decks[from.deck].frame !== layout.decks[to.deck].frame) continue;
      const [left, right] = from.deck < to.deck ? [from, to] : [to, from];
      const key = `${left.deck}:${right.deck}`;
      const candidate = candidates.get(key) ?? { left: left.deck, right: right.deck, pairs: new Map<number, Set<number>>() };
      const pair = candidate.pairs.get(left.card) ?? new Set<number>();
      pair.add(right.card);
      candidate.pairs.set(left.card, pair);
      candidates.set(key, candidate);
    }
    const links = new Map<number, Map<number, number[]>>();
    for (const { left, right, pairs } of candidates.values()) {
      const a = layout.decks[left], b = layout.decks[right];
      if (a.members.length !== b.members.length || pairs.size !== a.members.length) continue;
      const mapping = a.members.map((_, at) => pairs.get(at)?.size === 1 ? [...pairs.get(at)!][0] : -1);
      if (mapping.some((at) => at < 0) || new Set(mapping).size !== b.members.length) continue;
      const identity = (id: string) => { const item = items.get(id)!; return JSON.stringify([item.key ?? null, item.take ?? []]); };
      if (new Set(a.members.map(identity)).size !== a.members.length || new Set(b.members.map(identity)).size !== b.members.length
        || mapping.some((at, index) => identity(a.members[index]) !== identity(b.members[at]))) continue;
      const inverse = mapping.map((_, at) => mapping.indexOf(at));
      for (const [from, to, map] of [[left, right, mapping], [right, left, inverse]] as const) {
        const neighbours = links.get(from) ?? new Map<number, number[]>();
        neighbours.set(to, map);
        links.set(from, neighbours);
      }
    }
    for (let root = 0; root < layout.decks.length; root++) {
      if (this.groups[root]) continue;
      const maps = new Map<number, number[]>([[root, layout.decks[root].members.map((_, at) => at)]]);
      const pending = [root];
      let consistent = true;
      while (pending.length) {
        const current = pending.pop()!;
        for (const [next, mapping] of links.get(current) ?? []) {
          const positions = maps.get(current)!.map((at) => mapping[at]);
          const existing = maps.get(next);
          if (existing) { if (existing.some((at, row) => at !== positions[row])) consistent = false; }
          else { maps.set(next, positions); pending.push(next); }
        }
      }
      const decks = [...maps.keys()].sort((a, b) => a - b);
      if (!consistent) {
        for (const deck of decks) this.groups[deck] = { linked: { decks: [deck], rows: this.members[deck].map((_, at) => [at]) }, column: 0 };
        continue;
      }
      const linked = { decks, rows: this.members[root].map((_, row) => decks.map((deck) => maps.get(deck)![row])) };
      decks.forEach((deck, column) => { this.groups[deck] = { linked, column }; });
    }
  }

  /** The representative's member index, with explicit inspection of descendants respected. */
  front(deck: number, selected: string | null): number {
    const { linked, column } = this.groups[deck];
    const rows = linked.rows.map((positions) => positions.map((at, column) => this.members[linked.decks[column]][at]));
    const owners = selected === null ? undefined : this.owners.get(selected);
    const selectedDeck = linked.decks.find((index) => owners?.has(index));
    const member = selectedDeck === undefined ? null : this.members[selectedDeck][owners!.get(selectedDeck)!].id;
    return linked.rows[restingRow(rows, member)][column];
  }

  /** Corresponding members to raise together, including the requested member itself. */
  partners(deck: number, card: number): DeckPosition[] {
    const { linked, column } = this.groups[deck];
    const row = linked.rows.find((positions) => positions[column] === card)!;
    return linked.decks.map((deck, column) => ({ deck, card: row[column] }));
  }

  /** All closed representatives affected by inspecting this node or one of its ancestors. */
  affected(id: string): number[] {
    return [...new Set([...this.owners.get(id)?.keys() ?? []].flatMap((deck) => this.groups[deck].linked.decks))];
  }
}
