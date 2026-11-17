import type { CanvasGraph } from "./graph";
import { validateViewerApiBase } from "./route";

/** Visible canvas content, independent of recorded evidence and inspection capabilities. */
export interface ViewerDisplay {
  workflow_inputs: boolean;
  workflow_outputs: boolean;
  notes: boolean;
}

export interface ViewerConfig { display: ViewerDisplay }

export const DEFAULT_VIEWER_DISPLAY: Readonly<ViewerDisplay> = Object.freeze({
  workflow_inputs: true, workflow_outputs: true, notes: true,
});

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const names = Object.keys(DEFAULT_VIEWER_DISPLAY) as (keyof ViewerDisplay)[];

/** Explicit overrides are partial; absent settings retain the supplied defaults. */
export function viewerDisplay(overrides?: Partial<ViewerDisplay>, defaults: Readonly<ViewerDisplay> = DEFAULT_VIEWER_DISPLAY): ViewerDisplay {
  if (overrides !== undefined && (!record(overrides) || Object.keys(overrides).some((key) => !names.includes(key as keyof ViewerDisplay))
    || Object.values(overrides).some((value) => typeof value !== "boolean"))) throw new Error("The viewer display options are malformed.");
  return { ...defaults, ...overrides };
}

/** Native hosts and frozen exports supply complete, resolved configuration. */
export function parseViewerConfig(value: unknown): ViewerConfig {
  const display = record(value) ? value.display : null;
  if (!record(value) || Object.keys(value).some((key) => key !== "display") || !record(display)
    || Object.keys(display).length !== names.length || names.some((key) => typeof display[key] !== "boolean")) {
    throw new Error("The viewer configuration is incomplete or malformed.");
  }
  return { display: viewerDisplay(display as unknown as ViewerDisplay) };
}

export async function readViewerConfig(signal?: AbortSignal, apiBase = "/api", fetcher: typeof fetch = fetch): Promise<ViewerConfig> {
  const response = await fetcher(`${validateViewerApiBase(apiBase)}/viewer_config`, { signal, cache: "no-store" });
  // Older hosts have no configuration endpoint; their existing presentation stays enabled.
  if (response.status === 404) return { display: viewerDisplay() };
  if (!response.ok) throw new Error(`The viewer configuration could not be read (HTTP ${response.status}).`);
  let config: unknown;
  try { config = await response.json(); }
  catch { throw new Error("The viewer configuration is incomplete or malformed."); }
  return parseViewerConfig(config);
}

/** Filter content before measurement, retaining deliberately inspected hidden items temporarily. */
export function displayGraph(graph: CanvasGraph, display: Readonly<ViewerDisplay>, selected: string | null = null): CanvasGraph {
  if (display.workflow_inputs && display.workflow_outputs && display.notes) return graph;
  const nodes = graph.nodes.filter((node) => node.id === selected
    || !((node.value?.kind === "input" || node.kind === "boundary" && node.side === "input") && !display.workflow_inputs
      || (node.value?.kind === "output" || node.kind === "boundary" && node.side === "output") && !display.workflow_outputs));
  const visible = new Set([...nodes.map((node) => node.id), ...(graph.frames ?? []).map((frame) => frame.id)]);
  return {
    ...graph, nodes,
    edges: graph.edges.filter((edge) => visible.has(edge.source) && visible.has(edge.target)),
    ...(graph.frames ? { frames: graph.frames.map((frame) => ({ ...frame, nodes: frame.nodes.filter((id) => visible.has(id)) })) } : {}),
    ...(graph.notes ? { notes: graph.notes.filter((note) => display.notes || note.id === selected) } : {}),
  };
}

/** A deliberate reveal must stop materializing normally hidden content when selection leaves it. */
export function temporaryDisplaySelection(graph: CanvasGraph, selected: string | null, display: Readonly<ViewerDisplay>): boolean {
  const node = graph.nodes.find((node) => node.id === selected);
  return node?.value?.kind === "table" || node?.value?.kind === "let"
    || !display.workflow_inputs && (node?.value?.kind === "input" || node?.kind === "boundary" && node.side === "input")
    || !display.workflow_outputs && (node?.value?.kind === "output" || node?.kind === "boundary" && node.side === "output")
    || !display.notes && (graph.notes?.some((note) => note.id === selected) ?? false);
}
