/** Summarize recorded child states without declaring unfinished or unknown work successful. */
export function scopeState(states: readonly string[], plan: boolean): string {
  if (!states.length) return "empty";
  const has = (state: string) => states.includes(state);
  if (plan) {
    if (has("failed")) return "failed";
    if (has("blocked")) return "blocked";
    if (has("unexpanded")) return "unexpanded";
    if (has("maybe")) return "maybe";
    if (has("planned")) return "planned";
    if (states.every((state) => state === "absent")) return "absent";
    if (states.every((state) => state === "done" || state === "absent")) return "done";
    return "unknown";
  }
  if (has("running")) return "running";
  if (has("failed")) return "failed";
  if (has("blocked")) return "blocked";
  if (has("cancelled")) return "cancelled";
  if (has("pending") || has("planned") || has("unexpanded")) return "pending";
  if (states.every((state) => state === "skipped" || state === "absent")) return "skipped";
  if (states.every((state) => ["succeeded", "cached", "skipped", "absent"].includes(state))) return "succeeded";
  return "unknown";
}
