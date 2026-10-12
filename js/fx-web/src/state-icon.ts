/** The glyph a state shares across cards, step rows and badges: one circle slot, drawn in a 16-unit box. */
export type StateIcon = "succeeded" | "failed" | "running" | "waiting" | "cancelled" | "skipped" | "maybe" | "pending";

export function stateIcon(state: string): StateIcon {
  if (["succeeded", "cached", "done"].includes(state)) return "succeeded";
  if (["failed", "blocked"].includes(state)) return "failed";
  if (state === "running") return "running";
  if (state === "waiting") return "waiting";
  if (["cancelling", "cancelled", "interrupted"].includes(state)) return "cancelled";
  if (["skipped", "absent"].includes(state)) return "skipped";
  if (["maybe", "unexpanded"].includes(state)) return "maybe";
  return "pending";
}

export interface StateIconPart { tag: "circle" | "path"; attributes: Record<string, string | number> }

const disc: StateIconPart = { tag: "circle", attributes: { cx: 8, cy: 8, r: 8, class: "fx-state-disc" } };
const ring: StateIconPart = { tag: "circle", attributes: { cx: 8, cy: 8, r: 7, class: "fx-state-ring" } };
const mark = (d: string): StateIconPart => ({ tag: "path", attributes: { d, class: "fx-state-mark" } });

/** The shapes of a glyph; colours come from `.fx-state-icon[data-icon]` in canvas.css. */
export function stateIconParts(icon: StateIcon): StateIconPart[] {
  switch (icon) {
    case "succeeded": return [disc, mark("M4.75 8.25 7 10.5l4.25-4.75")];
    case "failed": return [disc, mark("M5.5 5.5l5 5m0-5-5 5")];
    case "running": return [
      { tag: "circle", attributes: { cx: 8, cy: 8, r: 7, class: "fx-state-track" } },
      { tag: "path", attributes: { d: "M8 1a7 7 0 0 1 7 7", class: "fx-state-arc" } },
      { tag: "circle", attributes: { cx: 8, cy: 8, r: 3.5, class: "fx-state-core" } },
    ];
    // A still clock (Lucide's `clock`, in this box): started, but its call waits for its route's slot.
    case "waiting": return [ring, { tag: "path", attributes: { d: "M8 4.5V8l2.25 1.5", class: "fx-state-hands" } }];
    case "cancelled": return [disc, mark("M5.25 10.75l5.5-5.5")];
    case "skipped": return [ring, mark("M3.05 12.95l9.9-9.9")];
    default: return [ring];
  }
}
