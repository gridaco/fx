/** Pure deck rules (spec/layout.md §5.3, §5.4): what the badge says and which member rests in front. */

export interface DeckMember {
  id: string;
  state: string;
  /** The item key; null without an item level. */
  key: string | null;
  /** The take number; null for a card with no take level. */
  take: number | null;
  /** Whether an instance outside the deck reads this member (§5.4 rule 4). */
  bound?: boolean;
}

const failed = new Set(["failed", "blocked"]);
const succeeded = new Set(["done", "succeeded", "cached"]);
const started = (state: string) => !["planned", "maybe", "absent", "pending", "skipped", "unexpanded"].includes(state);
const isTakeDeck = (members: DeckMember[]) => new Set(members.map((member) => member.key)).size === 1
  && members.some((member) => member.take !== null && member.take > 1);

/** "5 items", "3 takes", "2 items · 3 takes each", "· 1 failed". */
export function deckBadge(members: DeckMember[]) {
  const items = new Map<string, number>();
  for (const member of members) items.set(member.key ?? "", (items.get(member.key ?? "") ?? 0) + 1);
  const counts = [...items.values()];
  const takes = Math.max(...counts);
  const each = counts.every((count) => count === takes) ? `${takes} takes each` : `up to ${takes} takes`;
  const parts = items.size > 1
    ? [`${items.size} items`, ...(takes > 1 ? [each] : [])]
    : [`${members.length} ${isTakeDeck(members) ? "takes" : "items"}`];
  const failures = members.filter((member) => failed.has(member.state)).length;
  const notRun = members.filter((member) => member.state === "skipped").length;
  const waiting = members.filter((member) => member.state === "waiting").length;
  return {
    text: parts.join(" · "),
    failed: failures,
    detail: [...(failures ? [`${failures} failed`] : []), ...(waiting ? [`${waiting} waiting`] : []), ...(notRun ? [`${notRun} not run`] : [])],
  };
}

/**
 * The member that rests in front (§5.4): the selected one, else the last failed or blocked, else
 * the first running, else the first waiting for its route, else in a take deck the last take read
 * from outside it, else the last succeeded, else the first. A member that never started never
 * rests over one that did.
 */
export function restingFront(members: DeckMember[], selected: string | null): number {
  const last = (test: (member: DeckMember) => boolean) => {
    for (let at = members.length - 1; at >= 0; at--) if (test(members[at])) return at;
    return -1;
  };
  const rules = [
    () => members.findIndex((member) => member.id === selected),
    () => last((member) => failed.has(member.state)),
    () => members.findIndex((member) => member.state === "running"),
    () => members.findIndex((member) => member.state === "waiting"),
    () => isTakeDeck(members) ? last((member) => member.bound === true) : -1,
    () => last((member) => succeeded.has(member.state)),
    () => last((member) => started(member.state)),
  ];
  for (const rule of rules) {
    const found = rule();
    if (found >= 0) return found;
  }
  return 0;
}

/** Columns for an expanded deck: about as wide as the viewport's shape, never wider than n. */
export function expandedColumns(count: number, card: { width: number; height: number }, viewport: { width: number; height: number }) {
  const aspect = viewport.width > 0 && viewport.height > 0 ? viewport.width / viewport.height : 16 / 10;
  return Math.max(1, Math.min(count, Math.round(Math.sqrt(count * aspect * card.height / card.width))));
}

/** One mark of a deck's bookmark (§5.3): a member, or an even slice of them. */
export interface DeckTick {
  /** The members it stands for, by position: `first` to `last`, inclusive. */
  first: number;
  last: number;
  /** The worst state among them: a failure shows over work in progress over success. */
  tone: "failed" | "active" | "done" | "idle";
}

/** At most this many marks; more members share them in even slices. */
const DECK_TICKS = 16;

/** The bookmark's marks, top to bottom in member order. */
export function deckTicks(members: DeckMember[]): DeckTick[] {
  const count = Math.min(members.length, DECK_TICKS);
  return Array.from({ length: count }, (_, at) => {
    const first = Math.floor(at * members.length / count), last = Math.floor((at + 1) * members.length / count) - 1;
    const states = members.slice(first, last + 1).map((member) => member.state);
    const tone = states.some((state) => failed.has(state)) ? "failed"
      : states.some((state) => state === "running" || state === "waiting") ? "active"
        : states.some((state) => succeeded.has(state)) ? "done" : "idle";
    return { first, last, tone };
  });
}

/** Whether a mark stands for the member at `at`. */
export function tickHolds(tick: { first: number; last: number }, at: number) {
  return tick.first <= at && at <= tick.last;
}

/**
 * A bookmark at `step` per mark (§5.3): each mark's centre, and each member's row from the top.
 * The rows tile the marks; the first and last reach `reach` further out.
 */
export function bookmarkLayout(members: number, marks: number, step: number, reach: number) {
  const row = marks * step / members;
  return {
    height: marks * step,
    marks: Array.from({ length: marks }, (_, at) => at * step + step / 2),
    rows: Array.from({ length: members }, (_, at) => {
      const above = at === 0 ? reach : 0, below = at === members - 1 ? reach : 0;
      return { top: at * row - above, height: row + above + below };
    }),
  };
}

/** Back cards a closed deck shows behind its front one (§5.3). */
export const DECK_BACK = 3;

/**
 * How a closed deck rests (§5.3): four consecutive members around the front, two before it and
 * one after where they exist, the others hidden under the ends. `start` keeps a window that still
 * holds the front, so hovering a card raises it without moving the stack under the pointer.
 */
export function deckStack(count: number, front: number, kept?: number) {
  const span = Math.min(count, DECK_BACK + 1);
  const start = kept !== undefined && kept <= front && front < kept + span ? kept : Math.max(0, Math.min(front - 2, count - span));
  /** Each member's place in the stack, 0 the top-left; the hidden ones share an end's. */
  const slots = Array.from({ length: count }, (_, at) => Math.max(0, Math.min(at - start, span - 1)));
  const shown = slots.map((_, at) => at >= start && at < start + span);
  /** Back to front: the farther from the front, the deeper. */
  const order = [...slots.keys()].sort((a, b) => Math.abs(b - front) - Math.abs(a - front));
  return { start, slots, shown, order };
}
