import { expect, test } from "bun:test";
import { canvasGraph, type GraphDocument, type ViewerRun } from "../src/index";
import { step } from "./graph.test";

function candidates(): GraphDocument {
  return {
    kind: "fx-graph-v1", workflow: { id: "candidates", title: "Candidate selection" }, pending: [],
    estimate: { low_usd: 0, high_usd: 0, ceiling_usd: null },
    types: {
      render: { identity: "render@1", ports: { inputs: {}, outputs: { image: "image/png" }, params: {} } },
      review: { identity: "review@1", ports: { inputs: { image: "image/png" }, outputs: { report: "json" }, params: {} } },
      finish: { identity: "finish@1", ports: { inputs: { image: "image/png", report: "json" }, outputs: {}, params: {} } },
    },
    instances: [
      ...[1, 2, 3].map((take) => step(`render#${take}`, { uses: "render", take: [take] })),
      ...[1, 2, 3].map((take) => step(`review#${take}`, { uses: "review", take: [take], judges: `render#${take}`,
        reads: [`render#${take}`], bindings: [{ source: `render#${take}`, source_kind: "output", source_port: "image", target_port: "image" }] })),
      step("finish#1", { uses: "finish", reads: ["render#2", "review#2"], bindings: [
        { source: "render#2", source_kind: "output", source_port: "image", target_port: "image" },
        { source: "review#2", source_kind: "output", source_port: "report", target_port: "report" },
      ] }),
    ],
  };
}

function recorded(plan = candidates()): ViewerRun {
  return {
    kind: "fx-viewer-run-v1", workflow: plan.workflow, run_name: "local", state: "succeeded", stand_in: false,
    charged_usd: 0, estimate: null, inputs: {}, outputs: {}, artifacts: [], warnings: [],
    nodes: plan.instances.map((node) => ({
      id: node.id, path: node.path, step: node.step, take: node.take, key: node.key, title: node.path, uses: node.uses,
      state: node.take[0] === 3 ? "skipped" : "succeeded", reads: node.reads, judges: node.judges, needs: node.needs,
      bindings: node.bindings, ports: plan.types?.[node.uses].ports, with: {}, outputs: {}, cache: null, error: null,
      duration_ms: node.take[0] === 3 ? null : 0,
    })),
  };
}

test("candidate review wiring remains visible while only actual downstream reads establish consumption", () => {
  const view = recorded(), before = JSON.stringify(view);
  const graph = canvasGraph(view);
  expect(graph.edges.filter((edge) => edge.kinds.includes("judges"))).toHaveLength(3);
  expect(graph.edges.filter((edge) => edge.target.startsWith("instance:review#") && edge.kind === "data")).toHaveLength(3);
  expect(graph.edges.filter((edge) => edge.consumed).map((edge) => [edge.source, edge.target])).toEqual([
    ["instance:render#2", "instance:finish#1"], ["instance:review#2", "instance:finish#1"],
  ]);
  expect(graph.nodes.filter((node) => node.executed).map((node) => node.id)).toEqual([
    "instance:render#1", "instance:render#2", "instance:review#1", "instance:review#2", "instance:finish#1",
  ]);
  expect(JSON.stringify(view)).toBe(before);
  expect(canvasGraph(candidates()).edges.every((edge) => edge.consumed === undefined)).toBeTrue();
});

test("a skipped or failed consumer requires positive execution evidence and legacy reads keep that evidence", () => {
  for (const state of ["skipped", "failed", "succeeded", "pending"]) {
    const view = recorded();
    const finish = view.nodes.find((node) => node.id === "finish#1")!;
    Object.assign(finish, { state, duration_ms: null });
    delete finish.ports; delete finish.bindings;
    const reads = () => canvasGraph(view).edges.filter((edge) => edge.target === "instance:finish#1");
    expect(reads()).toHaveLength(2);
    expect(reads().every((edge) => edge.consumed === undefined)).toBeTrue();
    expect(canvasGraph(view).nodes.find((node) => node.id === "instance:finish#1")?.executed).toBeUndefined();
    finish.duration_ms = 0;
    expect(reads().every((edge) => edge.consumed === true)).toBeTrue();
    finish.duration_ms = null;
    finish.started_at = "2026-11-11T00:00:00.000Z";
    expect(reads().every((edge) => edge.consumed === true)).toBeTrue();
    delete finish.started_at;
    finish.cache = "hit";
    expect(reads().every((edge) => edge.consumed === true)).toBeTrue();
    finish.cache = null;
    finish.interrupted = true;
    expect(reads().every((edge) => edge.consumed === true)).toBeTrue();
  }
});

test("controls alone never establish consumption, and active consumers do not require dated starts", () => {
  const view = recorded();
  const finish = view.nodes.find((node) => node.id === "finish#1")!;
  finish.duration_ms = null;
  finish.state = "running";
  finish.needs = ["render#1"];
  expect(canvasGraph(view).edges.filter((edge) => edge.consumed).map((edge) => edge.source)).toEqual([
    "instance:render#2", "instance:review#2",
  ]);
  finish.state = "waiting";
  delete finish.bindings;
  finish.needs.push("render#2");
  const graph = canvasGraph(view);
  expect(graph.edges.find((edge) => edge.source === "instance:render#1" && edge.target === "instance:finish#1")?.consumed).toBeUndefined();
  expect(graph.edges.find((edge) => edge.source === "instance:render#2" && edge.target === "instance:finish#1"))
    .toMatchObject({ kind: "control", kinds: ["needs", "reads"], consumed: true });
});

test("an executed consumer's actual reads do not promote a stale planned binding", () => {
  const view = recorded();
  const finish = view.nodes.find((node) => node.id === "finish#1")!;
  finish.reads = ["render#1"];
  finish.bindings = [{ source: "render#3", source_kind: "output", source_port: "image", target_port: "image" }];
  const graph = canvasGraph(view);
  expect(graph.edges.find((edge) => edge.source === "instance:render#3" && edge.target === "instance:finish#1"))
    .toMatchObject({ kind: "data", kinds: ["reads"] });
  expect(graph.edges.filter((edge) => edge.consumed).map((edge) => edge.source)).toEqual(["instance:render#1"]);
});
