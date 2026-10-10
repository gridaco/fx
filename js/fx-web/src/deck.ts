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
  return {
    text: parts.join(" · "),
    short: `×${members.length}`,
    failed: failures,
    detail: [...(failures ? [`${failures} failed`] : []), ...(notRun ? [`${notRun} not run`] : [])],
  };
}

/**
 * The member that rests in front (§5.4): the selected one, else the last failed or blocked, else
 * the first running, else in a take deck the last take read from outside it, else the last
 * succeeded, else the first. A member that never started never rests over one that did.
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
