import { expect, test } from "bun:test";
import { canvasPortAnchor, layoutGraph, type CanvasConnection, type CanvasGraph, type CanvasPort, type CanvasStep } from "../src/index";

/** The topology and socket counts of the canonical viewer ports plan that broke multigraph routing. */
function recordedPortTopology(previews = false): CanvasGraph {
  const node = (id: string, inputs: string[], outputs: string[], settings: string[] = []): CanvasStep => ({
    id: `instance:${id}#1`, source_id: `${id}#1`, title: id, subtitle: "./nodes/ports.py#example", state: "planned", pending: false,
    ports: {
      inputs: inputs.map((name): CanvasPort => ({ name, type: "image", kind: "artifact" })),
      outputs: outputs.map((name): CanvasPort => ({ name, type: "image/png", kind: "artifact" })),
      settings: settings.map((name) => ({ name, type: "string" })),
    },
    ...(previews ? { preview: { kind: "image" as const, digest: "a".repeat(64), url: `/api/artifacts/${"a".repeat(64)}`, label: "Recorded output", count: 1 } } : {}),
  });
  const nodes = [
    node("seed", [], ["image"]), node("split", ["image"], ["color", "copy", "mask", "report", "pixel_count"]),
    node("merge", ["image", "mask"], ["image"]), node("same", ["left", "right"], ["report"]),
    node("list", ["images"], ["image"]), node("bundle", ["cool", "seed", "warm"], ["collection", "images"]),
    node("keyed", ["images"], ["image", "keys"]), node("tile['amber']", [], ["image"], ["color", "shape"]),
    node("tile['teal']", [], ["image"], ["color", "shape"]), node("wildcard", ["images"], ["image", "keys"]),
    node("settings", ["file_width", "label", "pixels", "width"], ["report"], ["literal"]), node("barrier", [], ["text"], ["message"]),
  ];
  nodes[1].ports!.outputs.at(-1)!.kind = "fact";
  nodes[10].ports!.inputs.forEach((port) => { port.kind = "parameter"; });
  const pairs = [
    ["seed", "image", "split", "image"],
    ["split", "color", "merge", "image"], ["split", "mask", "merge", "mask"],
    ["split", "color", "same", "left"], ["split", "copy", "same", "right"],
    ["split", "color", "list", "images"], ["split", "copy", "list", "images"], ["split", "mask", "list", "images"],
    ["merge", "image", "bundle", "warm"], ["seed", "image", "bundle", "seed"], ["split", "mask", "bundle", "cool"],
    ["bundle", "collection", "keyed", "images"], ["tile['amber']", "image", "wildcard", "images"], ["tile['teal']", "image", "wildcard", "images"],
    ["split", "color", "settings", "file_width"], ["split", "pixel_count", "settings", "pixels"],
    ["split", "report", "settings", "label"], ["split", "report", "settings", "width"],
  ];
  const edges: CanvasConnection[] = pairs.map(([from, source_port, to, target_port]) => {
    const source = `instance:${from}#1`, target = `instance:${to}#1`;
    const source_kind = source_port === "pixel_count" ? "fact" : "output";
    return { id: JSON.stringify(["data", source, source_kind, source_port, target, target_port]), source, target, kind: "data", kinds: ["reads"], source_port, target_port, source_kind };
  });
  for (const from of ["merge", "settings"]) {
    const source = `instance:${from}#1`, target = "instance:barrier#1";
    edges.push({ id: JSON.stringify(["control", source, target]), source, target, kind: "control", kinds: ["needs", "reads"] });
  }
  return { nodes, edges };
}

for (const previews of [false, true]) test(`canonical 12-node port topology lays out all 20 wires${previews ? " with previews" : ""}`, () => {
  const graph = recordedPortTopology(previews);
  const original = JSON.stringify(graph);
  const layout = layoutGraph(graph);
  expect(layout).toEqual(layoutGraph(graph));
  expect(layout.nodes).toHaveLength(12);
  expect(layout.edges).toHaveLength(20);
  expect(new Set(layout.edges.map((edge) => edge.id)).size).toBe(20);
  expect(new Set(layout.edges.map((edge) => JSON.stringify(edge.points))).size).toBe(20);
  const nodes = new Map(layout.nodes.map((node) => [node.id, node]));
  for (const edge of layout.edges) {
    expect(edge.points.every((point) => Number.isFinite(point.x) && Number.isFinite(point.y))).toBeTrue();
    if (edge.kind !== "data") continue;
    expect(edge.points[0]).toEqual(canvasPortAnchor(nodes.get(edge.source)!, "output", edge.source_port!, edge.source_kind === "fact" ? "fact" : "artifact"));
    expect(edge.points.at(-1)).toEqual(canvasPortAnchor(nodes.get(edge.target)!, "input", edge.target_port!));
  }
  for (const [index, a] of layout.nodes.entries()) for (const b of layout.nodes.slice(index + 1)) {
    expect(a.x + a.width <= b.x || b.x + b.width <= a.x || a.y + a.height <= b.y || b.y + b.height <= a.y).toBeTrue();
  }
  expect(JSON.stringify(graph)).toBe(original);
});
