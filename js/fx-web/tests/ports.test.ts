import { describe, expect, test } from "bun:test";
import { canvasGraph, canvasPortAnchor, isNodePorts, isPortBindings, layoutGraph, parseGraph, parseViewerRun, type GraphDocument, type NodePorts, type ViewerRun } from "../src/index";
import { step } from "./graph.test";

const outputPorts: NodePorts = { inputs: {}, outputs: { color: "image/png", mask: "image/png", report: "json" }, params: { size: { type: "integer" } } };
const inputPorts: NodePorts = { inputs: { image: "image", mask: "image?", images: "image[]", keyed: "image{}" }, outputs: { image: "image" }, params: { width: { type: "integer" }, pixels: { type: "integer" }, label: { type: "string" } } };

function portGraph(): GraphDocument {
  return {
    kind: "fx-graph-v1", workflow: { id: "ports", title: "Port connections" }, pending: [],
    estimate: { low_usd: 0, high_usd: 0, ceiling_usd: null },
    types: {
      source: { identity: "fx/source@1.0", ports: outputPorts },
      target: { identity: "fx/target@1.0", ports: inputPorts },
    },
    instances: [
      step("source#1", { uses: "source", bindings: [] }),
      step("target#1", { uses: "target", reads: ["source#1"], waiting_on: ["source#1"], needs: ["source#1"], with: { label: "Literal setting" }, bindings: [
        { source: "source#1", source_port: "color", target_port: "image", source_kind: "output" },
        { source: "source#1", source_port: "mask", target_port: "mask", source_kind: "output" },
        { source: "source#1", source_port: "color", target_port: "images", source_kind: "output" },
        { source: "source#1", source_port: "mask", target_port: "images", source_kind: "output" },
        { source: "source#1", source_port: "report", target_port: "width", source_kind: "output" },
        { source: "source#1", source_port: "pixel count", target_port: "pixels", source_kind: "fact" },
      ] }),
    ],
  };
}
function runFromPlan(plan: GraphDocument): ViewerRun {
  return {
    kind: "fx-viewer-run-v1", workflow: plan.workflow, run_name: "recorded", state: "succeeded", stand_in: false,
    charged_usd: 0, estimate: null, inputs: {}, outputs: {}, artifacts: [], warnings: [],
    nodes: plan.instances.map((node) => ({ id: node.id, path: node.path, title: node.path, uses: node.uses,
      state: "succeeded", reads: node.reads, with: node.with, outputs: {}, cache: null, error: null, duration_ms: 1,
      ports: plan.types?.[node.uses].ports, bindings: node.bindings, needs: node.needs, judges: node.judges })),
  };
}

describe("named port boundary", () => {
  test("accepts complete declarations and rejects invalid names, types, schemas and binding duplicates", () => {
    expect(isNodePorts(outputPorts)).toBeTrue();
    for (const ports of [
      { ...outputPorts, inputs: { "Bad-Name": "image" } },
      { ...outputPorts, outputs: { image: "arbitrary" } },
      { ...outputPorts, outputs: { image: "image[][]" } },
      { ...outputPorts, params: { size: "integer" } },
      { inputs: {}, outputs: {} },
      { ...outputPorts, unexpected: {} },
    ]) expect(isNodePorts(ports)).toBeFalse();
    const binding = { source: "source#1", source_port: "any fact name", target_port: "pixels", source_kind: "fact" };
    expect(isPortBindings([binding])).toBeTrue();
    expect(isPortBindings([{ ...binding, source_port: "" }])).toBeTrue();
    expect(isPortBindings([{ ...binding, source_kind: "output", source_port: "" }])).toBeFalse();
    expect(isPortBindings([{ ...binding, source_kind: "output", source_port: "Bad-Name" }])).toBeFalse();
    expect(isPortBindings([binding, { ...binding }])).toBeFalse();
    expect(isPortBindings([{ ...binding, target_port: "Bad-Name" }])).toBeFalse();
    expect(isPortBindings([{ ...binding, source_kind: "guess" }])).toBeFalse();
  });
  test("plan and recorded run boundaries preserve metadata and reject malformed optional fields", () => {
    const plan = portGraph();
    expect(parseGraph(plan)).toEqual(plan);
    const run = runFromPlan(plan);
    expect(parseViewerRun(run)).toEqual(run);
    const bad = structuredClone(plan) as any;
    bad.types.source.ports.inputs = { image: "secret-url" };
    expect(() => parseGraph(bad)).toThrow("does not match");
    const badRun = structuredClone(run) as any;
    badRun.nodes[0].bindings = null;
    expect(() => parseViewerRun(badRun)).toThrow("malformed");
  });
});

describe("named connection projection", () => {
  test("keeps parallel port wires, a separate control edge, facts and bound parameters", () => {
    const plan = portGraph();
    const original = JSON.stringify(plan);
    const graph = canvasGraph(plan);
    const source = graph.nodes[0];
    const target = graph.nodes[1];
    expect(graph.edges.filter((edge) => edge.kind === "data")).toHaveLength(6);
    expect(new Set(graph.edges.map((edge) => edge.id)).size).toBe(7);
    expect(graph.edges.filter((edge) => edge.kind === "control").map((edge) => edge.kinds)).toEqual([["needs"]]);
    expect(graph.edges.filter((edge) => edge.kind === "dependency")).toHaveLength(0);
    expect(source.ports?.outputs.find((port) => port.kind === "fact")).toEqual({ name: "pixel count", type: "fact", kind: "fact" });
    expect(target.ports?.inputs.filter((port) => port.kind === "parameter").map((port) => port.name)).toEqual(["pixels", "width"]);
    expect(target.ports?.settings.map((setting) => setting.name)).toEqual(["label"]);
    expect(target.ports?.inputs.filter((port) => port.name === "images")).toEqual([{ name: "images", type: "image[]", kind: "artifact" }]);
    expect(target.ports?.inputs.filter((port) => port.name === "keyed")).toEqual([{ name: "keyed", type: "image{}", kind: "artifact" }]);
    expect(JSON.stringify(plan)).toBe(original);
    expect(canvasGraph(parseViewerRun(runFromPlan(plan)))).toEqual({ ...graph, nodes: graph.nodes.map((node) => ({ ...node, state: "succeeded" })) });
  });
  test("retains unresolved dependencies without inventing data endpoints or sockets for legacy records", () => {
    const plan = portGraph();
    delete plan.types;
    plan.instances.forEach((node) => { delete node.bindings; });
    const legacy = canvasGraph(parseGraph(plan));
    expect(legacy.nodes.every((node) => node.ports === undefined)).toBeTrue();
    expect(legacy.edges).toHaveLength(1);
    expect(legacy.edges[0]).toMatchObject({ kind: "control", kinds: ["needs", "reads", "waiting"] });
    const unresolved = portGraph();
    unresolved.instances[1].bindings = [];
    unresolved.instances[1].needs = [];
    expect(canvasGraph(unresolved).edges).toHaveLength(1);
    expect(canvasGraph(unresolved).edges[0]).toMatchObject({ kind: "dependency", kinds: ["reads", "waiting"] });
    unresolved.instances[1].bindings = [{ source: "source#1", source_port: "not_declared", target_port: "image", source_kind: "output" }];
    expect(canvasGraph(unresolved).edges[0].kind).toBe("dependency");
  });
  test("attaches exact repeated instances and never substitutes a matching step path", () => {
    const plan = portGraph();
    plan.instances.splice(1, 0, step("source#2", { uses: "source", take: [2], bindings: [] }));
    plan.instances[2].bindings = [{ source: "source#2", source_port: "color", target_port: "image", source_kind: "output" }];
    const edge = canvasGraph(plan).edges.find((item) => item.kind === "data")!;
    expect(edge.source).toBe("instance:source#2");
    plan.instances[2].bindings![0].source = "source";
    expect(canvasGraph(plan).edges.some((item) => item.kind === "data")).toBeFalse();
  });
  test("an empty fact name keeps its distinct socket and exact data endpoint", () => {
    const plan = portGraph();
    plan.instances[1].bindings = [{ source: "source#1", source_port: "", target_port: "pixels", source_kind: "fact" }];
    const graph = canvasGraph(parseGraph(plan));
    expect(graph.nodes[0].ports?.outputs[0]).toEqual({ name: "", type: "fact", kind: "fact" });
    const layout = layoutGraph(graph);
    const edge = layout.edges.find((item) => item.kind === "data")!;
    expect(edge.points[0]).toEqual(canvasPortAnchor(layout.nodes[0], "output", "", "fact"));
    expect(edge.points[0].y).toBeGreaterThan(layout.nodes[0].y + 27);
  });
  test("shuffled declaration maps keep plan/run socket order, layout and binding identities", () => {
    const plan = structuredClone(portGraph());
    plan.types!.source.ports!.params = { zeta: { type: "string" }, ...plan.types!.source.ports!.params, alpha: { type: "number" } };
    plan.instances[1].bindings!.push(
      { source: "source#1", source_port: "report", target_port: "pixels", source_kind: "fact" },
      { source: "source#1", source_port: "alpha", target_port: "pixels", source_kind: "fact" },
    );
    const shuffled = structuredClone(plan);
    for (const type of Object.values(shuffled.types!)) for (const key of ["inputs", "outputs", "params"] as const) {
      type.ports![key] = Object.fromEntries(Object.entries(type.ports![key]).reverse());
    }
    const before = JSON.stringify(shuffled);
    const expected = canvasGraph(plan);
    const projected = canvasGraph(shuffled);
    expect(projected).toEqual(expected);
    expect(projected.nodes[0].ports!.settings.map((setting) => setting.name)).toEqual(["alpha", "size", "zeta"]);
    expect(projected.nodes[0].ports!.outputs.map((port) => [port.name, port.kind])).toEqual([
      ["alpha", "fact"], ["color", "artifact"], ["mask", "artifact"], ["pixel count", "fact"], ["report", "artifact"], ["report", "fact"],
    ]);
    const run = canvasGraph(runFromPlan(shuffled));
    // The state is the only plan/run difference in this declaration-only case.
    run.nodes.forEach((node, index) => { node.state = expected.nodes[index].state; });
    expect(run).toEqual(expected);
    const expectedLayout = layoutGraph(expected);
    expect(layoutGraph(projected)).toEqual(expectedLayout);
    expect(layoutGraph(run)).toEqual(expectedLayout);
    const placed = layoutGraph(projected);
    for (const node of placed.nodes) for (const port of node.ports!.outputs) {
      expect(canvasPortAnchor(node, "output", port.name, port.kind)).toEqual(canvasPortAnchor(expectedLayout.nodes.find((item) => item.id === node.id)!, "output", port.name, port.kind));
    }
    const reversedBindings = structuredClone(shuffled);
    reversedBindings.instances[1].bindings!.reverse();
    expect(canvasGraph(reversedBindings).nodes[0].ports!.outputs).toEqual(expected.nodes[0].ports!.outputs);
    expect(JSON.stringify(shuffled)).toBe(before);
  });
});

test("port layout preserves parallel edges at their exact sockets and stable nonoverlapping geometry", () => {
  const graph = canvasGraph(portGraph());
  const original = JSON.stringify(graph);
  const layout = layoutGraph(graph);
  expect(layout).toEqual(layoutGraph(graph));
  expect(JSON.stringify(graph)).toBe(original);
  expect(layout.edges).toHaveLength(7);
  const source = layout.nodes[0], target = layout.nodes[1];
  expect(target.x).toBeGreaterThan(source.x + source.width);
  for (const edge of layout.edges.filter((item) => item.kind === "data")) {
    expect(edge.points[0]).toEqual(canvasPortAnchor(source, "output", edge.source_port!, edge.source_kind === "fact" ? "fact" : "artifact"));
    expect(edge.points.at(-1)).toEqual(canvasPortAnchor(target, "input", edge.target_port!));
    expect(edge.points.every((point) => Number.isFinite(point.x) && Number.isFinite(point.y))).toBeTrue();
  }
  expect(canvasPortAnchor(source, "output", "unknown")).toBeUndefined();
  expect(canvasPortAnchor(source, "output", "color")!.y).not.toBe(canvasPortAnchor(source, "output", "mask")!.y);
  expect(layout.edges.find((edge) => edge.kind === "control")!.points[0].y).toBeLessThan(canvasPortAnchor(source, "output", "color")!.y);
});
