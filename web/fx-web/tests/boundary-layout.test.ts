import { expect, test } from "bun:test";
import { canvasNodeSize, canvasPortAnchor, layoutGraph, type CanvasGraph, type CanvasStep, type LayoutReport, type ValueView } from "../src/index";
import { canvasGraph } from "../src/graph";
import { nestedPlan } from "./scopes.test";

function value(char: string, kind: ValueView["kind"], name: string, data: unknown = name, changes: Partial<CanvasStep> = {}): CanvasStep {
  const id = `value:${char.repeat(64)}`;
  const port = { name, type: "value", kind: "value" as const };
  return { id, source_id: id, address: `value:root:${kind}:${name}`, kind: "value", value_id: id,
    value: { id, scope_id: null, kind, name, state: "known", data: { value: data } },
    title: name, subtitle: kind, state: "known", pending: false,
    ports: { inputs: kind === "input" || kind === "table" ? [] : [port], outputs: kind === "output" ? [] : [port], settings: [] }, ...changes };
}

function step(id: string): CanvasStep {
  return { id, source_id: id, address: id, title: id, subtitle: "local", state: "succeeded", pending: false,
    ports: { inputs: [{ name: "prompt", type: "text", kind: "parameter" }, { name: "image", type: "image", kind: "artifact" }],
      outputs: [{ name: "image", type: "image", kind: "artifact" }], settings: [] } };
}

const contains = (parent: { x: number; y: number; width: number; height: number }, child: typeof parent) =>
  child.x >= parent.x && child.y >= parent.y && child.x + child.width <= parent.x + parent.width && child.y + child.height <= parent.y + parent.height;
const overlaps = (left: { x: number; y: number; width: number; height: number }, right: typeof left) =>
  left.x < right.x + right.width && left.x + left.width > right.x && left.y < right.y + right.height && left.y + left.height > right.y;

test("boundary panels pack mixed values independently of processing rows and keep every socket connected", () => {
  const prompt = value("a", "input", "prompt", "A detailed image description. ".repeat(80));
  const seed = value("b", "input", "seed", 42);
  const reference = value("c", "input", "reference", "reference.png", { value_media: true });
  const result = value("d", "output", "result", "result.png", { value_media: true });
  const manifest = value("e", "output", "manifest", { count: 2 });
  const shared = [value("f", "table", "sizes", { width: 1024 }), value("0", "let", "brief", "A rendered brief")];
  const draw = step("draw"), finish = step("finish");
  const boundaryNodes = [prompt, seed, reference, result, manifest];
  const graph: CanvasGraph = { nodes: [draw, finish, ...shared, ...boundaryNodes], edges: [
    { id: "prompt", source: prompt.id, target: draw.id, kind: "data", kinds: ["value"], source_port: "prompt", target_port: "prompt" },
    { id: "reference", source: reference.id, target: draw.id, kind: "data", kinds: ["value"], source_port: "reference", target_port: "image" },
    { id: "result", source: finish.id, target: result.id, kind: "data", kinds: ["value"], source_port: "image", target_port: "result" },
  ], notes: [{ id: "note:root:guide", scope_id: null, name: "guide", about: null, format: "text", text: "Instructions", frame_id: null }] };
  const report: LayoutReport = { kind: "fx-layout-report-v1", file: null, revision: null, state: "none", cursor: null, diagnostics: [],
    order: ["draw", "finish"], cells: { draw: { column: 0, row: 0, source: "automatic" }, finish: { column: 1, row: 1, source: "automatic" } },
    notes: { "note:root:guide": { column: 0, row: 2, source: "automatic" } },
    values: Object.fromEntries([...boundaryNodes, ...shared].map((node, row) => [node.id, { column: 0, row: row + 10, source: "automatic" as const }])) };
  const noteSizes = new Map([["note:root:guide", { width: 360, height: 720 }]]);
  const before = JSON.stringify({ graph, report });
  const layout = layoutGraph(graph, report, noteSizes);
  const baseline = layoutGraph({ ...graph, nodes: graph.nodes.filter((node) => !boundaryNodes.includes(node)), edges: [] }, report, noteSizes);
  expect(layout.boundary_groups?.map((group) => [group.side, group.title, group.nodes.length])).toEqual([["input", "Inputs", 3], ["output", "Outputs", 2]]);
  const at = (id: string) => layout.nodes.find((node) => node.id === id)!;
  for (const group of layout.boundary_groups!) {
    const cards = group.nodes.map(at);
    expect(cards[0].y - group.y).toBe(40);
    for (const [index, card] of cards.entries()) {
      expect(contains(group, card)).toBeTrue();
      expect(card.x - group.x).toBe(20);
      if (index) expect(card.y - cards[index - 1].y - cards[index - 1].height).toBe(16);
      expect(card).toMatchObject({ ...canvasNodeSize(graph.nodes.find((node) => node.id === card.id)!) });
    }
    for (const outside of [...layout.nodes.filter((node) => !group.nodes.includes(node.id)), ...layout.notes!]) {
      expect(overlaps(group, outside)).toBeFalse();
    }
    expect(layout.width).toBeGreaterThan(group.x + group.width);
    expect(layout.height).toBeGreaterThan(group.y + group.height);
  }
  for (const node of [draw, finish, ...shared]) {
    const previous = baseline.nodes.find((item) => item.id === node.id)!;
    expect(at(node.id).y).toBe(previous.y);
    expect(at(node.id).x - at(draw.id).x).toBe(previous.x - baseline.nodes.find((item) => item.id === draw.id)!.x);
  }
  expect(layout.edges).toHaveLength(graph.edges.length);
  for (const edge of layout.edges) {
    expect(edge.points[0]).toEqual(canvasPortAnchor(at(edge.source), "output", edge.source_port!));
    expect(edge.points.at(-1)).toEqual(canvasPortAnchor(at(edge.target), "input", edge.target_port!));
  }
  expect(JSON.stringify({ graph, report })).toBe(before);
});

test("an opened import groups its own public values while inline group values retain their frame", () => {
  const view = nestedPlan(), scope = "scope:batch.item#1";
  const input = value("a", "input", "image"), output = value("b", "output", "hero"), internal = value("c", "input", "detail");
  view.value_views = [
    { ...input.value!, scope_id: scope }, { ...output.value!, scope_id: scope },
    { ...internal.value!, scope_id: "scope:batch#1" },
  ];
  const root = layoutGraph(canvasGraph(view));
  expect(root.boundary_groups).toBeUndefined();
  expect(root.frames.find((frame) => frame.id === "scope:batch#1")?.nodes).toContain(internal.id);
  const opened = layoutGraph(canvasGraph(view, scope));
  expect(opened.boundary_groups?.map((group) => group.nodes)).toEqual([[input.id], [output.id]]);
  expect(opened.nodes.filter((node) => node.kind === "boundary")).toEqual([]);
  expect(opened.nodes.some((node) => node.id === internal.id)).toBeFalse();
});

test("empty and legacy canvases keep their geometry without adding presentation containers", () => {
  expect(layoutGraph({ nodes: [], edges: [] })).toEqual({ nodes: [], notes: [], edges: [], frames: [], decks: [], width: 20, height: 20 });
  const graph: CanvasGraph = { nodes: [step("first"), step("second")], edges: [] };
  const layout = layoutGraph(graph);
  expect(layout.boundary_groups).toBeUndefined();
  expect(layout.nodes.map(({ id, x, y, width, height }) => ({ id, x, y, width, height }))).toEqual([
    { id: "first", x: 20, y: 20, width: 360, height: 399 },
    { id: "second", x: 480, y: 20, width: 360, height: 399 },
  ]);
});
