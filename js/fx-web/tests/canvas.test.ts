import { expect, test } from "bun:test";
import { BADGE_BAND, DECK_OFFSET, canvasEdgePath, canvasNodeGeometry, canvasPreviewReserved, canvasNodeSize, canvasPortAnchor, deckBadge, layoutGraph, restingFront, type CanvasGraph, type CanvasStep, type LayoutReport } from "../src/index";

function node(id: string, preview = false, address = id): CanvasStep {
  return {
    id, source_id: id, address, title: id, subtitle: "./nodes.py#recolor", state: "succeeded", pending: false,
    ...(preview ? { preview: { kind: "image" as const, digest: "a".repeat(64), url: `/api/artifacts/${"a".repeat(64)}`, label: "Recolored image", count: 1 } } : {}),
  };
}

function take(id: string, key: string | null, number: number, state = "done") { return { id, key, take: number, state }; }

function report(cells: Record<string, [number, number]>, order: string[] = []): LayoutReport {
  return { kind: "fx-layout-report-v1", file: null, revision: null, state: "none", cursor: null, diagnostics: [], order,
    cells: Object.fromEntries(Object.entries(cells).map(([address, [column, row]]) => [address, { column, row, source: "automatic" }])) };
}

test("metadata-only nodes retain their size; image nodes reserve space before loading", () => {
  expect(canvasNodeSize(node("planned"))).toEqual({ width: 250, height: 106 });
  expect(canvasNodeSize(node("rendered", true))).toEqual({ width: 250, height: 246 });
});

test("declared port rows and settings reserve space above an image preview", () => {
  const rich: CanvasStep = { ...node("many-inputs", true), ports: {
    inputs: Array.from({ length: 12 }, (_, index) => ({ name: `image_${index}`, type: "image", kind: "artifact" })),
    outputs: [{ name: "image", type: "image", kind: "artifact" }],
    settings: [{ name: "scale", type: "number" }],
  } };
  const placed = layoutGraph({ nodes: [rich], edges: [] }).nodes[0];
  const geometry = canvasNodeGeometry(rich);
  const last = canvasPortAnchor(placed, "input", "image_11")!;
  expect(last.y + 16).toBeLessThan(placed.y + geometry.settings_y);
  expect(geometry.settings_y + 10).toBeLessThan(geometry.preview_y);
  expect(geometry.preview_y + 140).toBeLessThan(geometry.height - 22);
});

test("mixed image and metadata nodes remain separated with edges landing at their borders", () => {
  const graph: CanvasGraph = {
    nodes: [node("input", true), node("recolor", true), node("wait")],
    edges: [
      { id: "input-recolor", source: "input", target: "recolor", kinds: ["reads"] },
      { id: "input-wait", source: "input", target: "wait", kinds: ["reads"] },
    ],
  };
  const original = JSON.stringify(graph);
  const cells = report({ input: [0, 0], recolor: [1, 0], wait: [1, 1] });
  const layout = layoutGraph(graph, cells);
  expect(layout).toEqual(layoutGraph(graph, cells));
  expect(JSON.stringify(graph)).toBe(original);
  const source = layout.nodes.find((item) => item.id === "input")!;
  for (const id of ["recolor", "wait"]) {
    const target = layout.nodes.find((item) => item.id === id)!;
    const edge = layout.edges.find((item) => item.id === `input-${id}`)!;
    expect(target.x).toBeGreaterThan(source.x + source.width);
    expect(edge.points[0].x).toBeCloseTo(source.x + source.width);
    expect(edge.points.at(-1)!.x).toBeCloseTo(target.x);
  }
  const recolor = layout.nodes.find((item) => item.id === "recolor")!;
  const wait = layout.nodes.find((item) => item.id === "wait")!;
  expect(recolor.y + recolor.height <= wait.y || wait.y + wait.height <= recolor.y).toBeTrue();
});

test("nested group frames contain their direct children without enclosing an outside consumer", () => {
  const graph: CanvasGraph = {
    nodes: [node("a"), node("b"), node("c"), node("outside")],
    edges: [
      { id: "ab", source: "a", target: "b", kinds: ["reads"] },
      { id: "bc", source: "b", target: "c", kinds: ["reads"] },
      { id: "out", source: "c", target: "outside", kinds: ["reads"] },
    ],
    frames: [
      { id: "outer", title: "Assembly", nodes: ["a", "b"] },
      { id: "inner", title: "Finish", nodes: ["c"], parent: "outer" },
    ],
  };
  const original = JSON.stringify(graph);
  const layout = layoutGraph(graph);
  const contains = (frame: { x: number; y: number; width: number; height: number }, child: typeof frame) =>
    child.x >= frame.x && child.y >= frame.y && child.x + child.width <= frame.x + frame.width && child.y + child.height <= frame.y + frame.height;
  const outer = layout.frames.find((frame) => frame.id === "outer")!;
  const inner = layout.frames.find((frame) => frame.id === "inner")!;
  for (const id of ["a", "b", "c"]) expect(contains(outer, layout.nodes.find((item) => item.id === id)!)).toBeTrue();
  expect(contains(outer, inner)).toBeTrue();
  expect(contains(inner, layout.nodes.find((item) => item.id === "c")!)).toBeTrue();
  expect(contains(outer, layout.nodes.find((item) => item.id === "outside")!)).toBeFalse();
  expect(layout).toEqual(layoutGraph(graph));
  expect(JSON.stringify(graph)).toBe(original);
});

test("workflow interface values attach by name and workflow cards reserve room for navigation", () => {
  const inputs: CanvasStep = { ...node("inputs"), kind: "boundary", ports: {
    inputs: [], outputs: [{ name: "theme", type: "string", kind: "parameter" }], settings: [],
  } };
  const workflow: CanvasStep = { ...node("workflow"), kind: "workflow", scope_id: "scope:tile#1", child_count: 2, ports: {
    inputs: [{ name: "theme", type: "string", kind: "parameter" }], outputs: [{ name: "result", type: "unknown", kind: "value" }], settings: [],
  } };
  const outputs: CanvasStep = { ...node("outputs"), kind: "boundary", ports: {
    inputs: [{ name: "result", type: "unknown", kind: "value" }], outputs: [], settings: [],
  } };
  const graph: CanvasGraph = { nodes: [inputs, workflow, outputs], edges: [
    { id: "into", source: inputs.id, target: workflow.id, kind: "data", kinds: ["reads"], source_port: "theme", target_port: "theme", source_kind: "output" },
    { id: "out", source: workflow.id, target: outputs.id, kind: "data", kinds: ["reads"], source_port: "result", target_port: "result", source_kind: "output" },
  ] };
  const layout = layoutGraph(graph);
  const placed = layout.nodes.find((item) => item.id === workflow.id)!;
  expect(placed.height).toBeGreaterThan(canvasNodeSize({ ...workflow, kind: undefined }).height);
  expect(layout.edges[0].points[0]).toEqual(canvasPortAnchor(layout.nodes[0], "output", "theme"));
  expect(layout.edges[1].points[0]).toEqual(canvasPortAnchor(placed, "output", "result"));
  expect(layout.edges[1].points.at(-1)).toEqual(canvasPortAnchor(layout.nodes[2], "input", "result"));
});

test("a forward edge is one curve that meets both sockets horizontally", () => {
  const points = [{ x: 840, y: 181 }, { x: 940, y: 168 }];
  const original = JSON.stringify(points);
  const path = canvasEdgePath(points);
  expect(path).toMatch(/^M [\d.]+ [\d.]+ C [\d.]+ [\d.]+ [\d.]+ [\d.]+ [\d.]+ [\d.]+$/);
  const [x0, y0, x1, y1, x2, y2, x3, y3] = path.match(/[\d.]+/g)!.map(Number);
  expect({ x: x0, y: y0 }).toEqual(points[0]);
  expect({ x: x3, y: y3 }).toEqual(points.at(-1)!);
  expect(y1).toBe(y0);
  expect(y2).toBe(y3);
  expect(x1).toBeGreaterThan(x0);
  expect(x2).toBeLessThan(x3);
  for (let t = 0; t <= 1; t += 0.05) {
    const y = (1 - t) ** 3 * y0 + 3 * (1 - t) ** 2 * t * y1 + 3 * (1 - t) * t ** 2 * y2 + t ** 3 * y3;
    expect(y).toBeGreaterThanOrEqual(Math.min(y0, y3));
    expect(y).toBeLessThanOrEqual(Math.max(y0, y3));
  }
  expect(canvasEdgePath([{ x: 840, y: 147 }, { x: 940, y: 134 }])).not.toBe(path);
  expect(JSON.stringify(points)).toBe(original);
});

test("long and backward routes round their detours without dropping them or producing invalid coordinates", () => {
  const long = [{ x: 0, y: 0 }, { x: 16, y: 0 }, { x: 100, y: -80 }, { x: 300, y: -80 }, { x: 384, y: 0 }, { x: 400, y: 0 }];
  const path = canvasEdgePath(long);
  expect(path).toContain("Q 100 -80");
  expect(path).toContain("Q 300 -80");
  expect(path.endsWith("L 400 0")).toBeTrue();
  expect(canvasEdgePath([...long.slice(0, 3), long[2], ...long.slice(3)])).toBe(path);
  expect(path).not.toMatch(/NaN|Infinity/);
  expect(canvasEdgePath([{ x: 100, y: 10 }, { x: 116, y: 10 }, { x: 80, y: -100 }, { x: -16, y: 10 }, { x: 0, y: 10 }])).toContain("Q 80 -100");
  expect(canvasEdgePath([])).toBe("");
  expect(canvasEdgePath([{ x: 1, y: 2 }])).toBe("M 1 2");
});

test("cards go in the served cells; a deck stacks at most three back cards above a badge band", () => {
  const members = ["d4", "d1", "d3", "d2", "d5"].map((id) => node(id, false, "draw"));
  const graph: CanvasGraph = { nodes: [node("seed"), ...members], edges: [] };
  const layout = layoutGraph(graph, report({ seed: [0, 0], draw: [1, 0] }, ["seed", "d1", "d2", "d3", "d4", "d5"]));
  const seed = layout.nodes.find((item) => item.id === "seed")!;
  const deck = layout.decks[0];
  expect(deck.members).toEqual(["d1", "d2", "d3", "d4", "d5"]);
  expect(deck.x).toBe(seed.x + seed.width + 100);
  const placed = deck.members.map((id) => layout.nodes.find((item) => item.id === id)!);
  expect(placed.map((item) => item.x - deck.x)).toEqual([0, 1, 2, 3, 3].map((step) => step * DECK_OFFSET));
  expect(placed[0].y).toBe(deck.y + BADGE_BAND);
  expect(deck.width).toBe(250 + 3 * DECK_OFFSET);
});

test("the frames of one group share their tracks, so its frame cards are the same size", () => {
  const graph: CanvasGraph = {
    nodes: [node("a.draw", true, "item.draw"), node("a.check", false, "item.check"), node("b.draw", false, "item.draw"), node("b.check", false, "item.check")],
    edges: [],
    frames: [
      { id: "a", title: "a", nodes: ["a.draw", "a.check"], address: "item" },
      { id: "b", title: "b", nodes: ["b.draw", "b.check"], address: "item" },
    ],
  };
  const layout = layoutGraph(graph, report({ item: [0, 0], "item.draw": [0, 0], "item.check": [1, 0] }));
  const [a, b] = ["a", "b"].map((id) => layout.frames.find((frame) => frame.id === id)!);
  expect([a.width, a.height]).toEqual([b.width, b.height]);
  expect(layout.decks.map((deck) => deck.kind)).toEqual(["frame"]);
});

test("without a report, slots wait in one row in projection order", () => {
  const layout = layoutGraph({ nodes: [node("a"), node("b")], edges: [] });
  expect(layout.nodes.map((item) => item.y)).toEqual([20, 20]);
  expect(layout.nodes[1].x).toBeGreaterThan(layout.nodes[0].x + layout.nodes[0].width);
});

test("a deck's badge counts items and takes, and the resting card follows §5.4", () => {
  expect(deckBadge([take("a", "x", 1), take("b", "y", 1)]).text).toBe("2 items");
  expect(deckBadge([take("a", null, 1), take("b", null, 2), take("c", null, 3)]).text).toBe("3 takes");
  expect(deckBadge([take("a", "x", 1), take("b", "x", 2), take("c", "y", 1), take("d", "y", 2)]).text).toBe("2 items · 2 takes each");
  const failed = deckBadge([take("a", "x", 1, "failed"), take("b", "y", 1)]);
  expect([failed.failed, failed.detail]).toEqual([1, ["1 failed"]]);
  const members = [take("a", "x", 1), take("b", "y", 1, "failed"), take("c", "z", 1), take("d", "w", 1, "planned")];
  expect(restingFront(members, null)).toBe(1);
  expect(restingFront(members, "c")).toBe(2);
  expect(restingFront([take("a", "x", 1), take("b", "y", 1), take("c", "z", 1, "planned")], null)).toBe(1);
});

test("the resting card takes the first running member and, in a take deck, the take read from outside", () => {
  const running = [take("a", "x", 1, "running"), take("b", "y", 1, "running"), take("c", "z", 1)];
  expect(restingFront(running, null)).toBe(0);
  const takes = [{ ...take("a", null, 1), bound: true }, take("b", null, 2), take("c", null, 3)];
  expect(restingFront(takes, null)).toBe(0);
  expect(restingFront([take("a", null, 1), take("b", null, 2, "absent")], null)).toBe(0);
  expect(deckBadge([take("a", "x", 1), take("b", "x", 2), take("c", "y", 1)]).text).toBe("2 items · up to 2 takes");
});

test("absent instances never join a deck, and an image output reserves its preview before it exists", () => {
  const absent = { ...node("b#1", false, "b"), state: "absent" };
  const layout = layoutGraph({ nodes: [node("a#1", false, "b"), absent], edges: [] }, report({ b: [0, 0] }));
  expect([layout.nodes.map((item) => item.id), layout.decks]).toEqual([["a#1"], []]);
  const only = layoutGraph({ nodes: [absent, { ...absent, id: "b#2", source_id: "b#2" }], edges: [] }, report({ b: [0, 0] }));
  expect(only.nodes.map((item) => item.id)).toEqual(["b#1"]);
  const ports = { inputs: [], outputs: [{ name: "image", type: "image/png", kind: "artifact" as const }], settings: [] };
  expect(canvasPreviewReserved({ ...node("draw"), ports })).toBe(true);
  expect(canvasNodeSize({ ...node("draw"), ports })).toEqual(canvasNodeSize({ ...node("draw", true), ports }));
});
