import { expect, test } from "bun:test";
import { canvasEdgePath, canvasNodeGeometry, canvasNodeSize, canvasPortAnchor, layoutGraph, type CanvasGraph, type CanvasStep } from "../src/index";

function node(id: string, preview = false): CanvasStep {
  return {
    id, source_id: id, title: id, subtitle: "./nodes.py#recolor", state: "succeeded", pending: false,
    ...(preview ? { preview: { kind: "image" as const, digest: "a".repeat(64), url: `/api/artifacts/${"a".repeat(64)}`, label: "Recolored image", count: 1 } } : {}),
  };
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
  const layout = layoutGraph(graph);
  expect(layout).toEqual(layoutGraph(graph));
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
  const outer = layout.frames!.find((frame) => frame.id === "outer")!;
  const inner = layout.frames!.find((frame) => frame.id === "inner")!;
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

test("nearby socket curves do not inherit a center-route peak and meet both sockets horizontally", () => {
  const points = [{ x: 840, y: 181 }, { x: 856, y: 181 }, { x: 890, y: 144 }, { x: 924, y: 168 }, { x: 940, y: 168 }];
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
  expect(canvasEdgePath([{ x: 840, y: 147 }, { x: 856, y: 147 }, { x: 890, y: 136 }, { x: 924, y: 134 }, { x: 940, y: 134 }])).not.toBe(path);
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
