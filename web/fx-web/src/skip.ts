import type { RunNode } from "./index";
import type { WorkflowScope } from "./scopes";

/** Engine-recorded causal facts, independent of authored prose and browser presentation. */
export interface SkipReason { code: string; sources: string[] }
export interface SkipExplanation {
  message: string;
  related: { id: string; label: string }[];
}

/** Unknown future codes remain readable, while malformed payloads fail the transport boundary. */
export function isSkipReason(value: unknown): value is SkipReason {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const reason = value as Record<string, unknown>;
  return Object.keys(reason).length === 2 && typeof reason.code === "string" && /^[a-z][a-z0-9_]*$/.test(reason.code)
    && Array.isArray(reason.sources) && reason.sources.every((source) => typeof source === "string" && source.length > 0)
    && new Set(reason.sources).size === reason.sources.length;
}

/** Explain only recorded causes; source names and take numbers come from the recorded inventory. */
export function skipExplanation(reason: SkipReason | undefined, nodes: readonly RunNode[]): SkipExplanation {
  if (!reason) return { message: "Reason not recorded.", related: [] };
  const inventory = new Map(nodes.map((node) => [node.id, node]));
  const sources = reason.sources.flatMap((id) => inventory.get(id) ?? []);
  // A missing related instance makes the asserted causal relationship unverifiable here.
  if (sources.length !== reason.sources.length) return { message: "Skip reason unavailable in this viewer.", related: [] };
  const take = sources[0]?.take?.at(-1);
  let message: string;
  switch (reason.code) {
    case "earlier_take_accepted": message = take === undefined ? "This take wasn’t needed because an earlier take was accepted." : `This take wasn’t needed because take ${take} was accepted.`; break;
    case "earlier_take_settled": message = "An earlier take already settled this sequence."; break;
    case "condition_false": message = "The condition for running this step was not met."; break;
    case "upstream_failed": message = "A required upstream step failed."; break;
    case "upstream_absent": message = "A required upstream result was unavailable."; break;
    case "rejected": message = "The candidate was rejected and the workflow skips this path."; break;
    case "assertion_skip": message = "A workflow check failed and was configured to skip this step."; break;
    case "no_candidate": message = "None of the supplied candidates was available."; break;
    default: return { message: "Skip reason unavailable in this viewer.", related: [] };
  }
  return { message, related: sources.map((node) => ({ id: node.id,
    label: `${node.title || node.path}${node.take?.length ? ` · take ${node.take.join(".")}` : ""}` })) };
}

/** A bounded SVG label; the card's accessible name retains the complete explanation. */
export function skipPreviewLines(message: string, columns = 43, maximum = 3): string[] {
  const words = message.split(/\s+/).filter(Boolean), lines: string[] = [];
  let current = "";
  for (const word of words) {
    if (current && [...`${current} ${word}`].length > columns) { lines.push(current); current = ""; }
    current = current ? `${current} ${word}` : word;
  }
  if (current) lines.push(current);
  return lines.slice(0, maximum).map((line, at) => {
    const truncated = [...line].length > columns || at === maximum - 1 && lines.length > maximum;
    return truncated ? [...line].slice(0, columns - 1).join("") + "…" : line;
  });
}

/** Open only recorded workflow ancestors; group frames stay visible in their parent's canvas. */
export function skipSourceScopes(id: string, scopes: readonly WorkflowScope[] = []): string[] {
  const byId = new Map(scopes.map((scope) => [scope.id, scope]));
  let scope = scopes.find((scope) => scope.nodes.includes(id));
  const ancestors: string[] = [], seen = new Set<string>();
  while (scope && !seen.has(scope.id)) {
    seen.add(scope.id);
    if (scope.kind === "workflow") ancestors.unshift(scope.id);
    scope = scope.parent === null ? undefined : byId.get(scope.parent);
  }
  return ancestors;
}
