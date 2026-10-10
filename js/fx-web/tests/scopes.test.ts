import { describe, expect, test } from "bun:test";
import {
  canvasGraph, flatCanvasGraph, isInterfaceBindings, isWorkflowScopes, navigationScopes, parseGraph, parseViewerRun,
  ViewerController, canvasPortAnchor, layoutGraph, type GraphDocument, type InterfaceBinding, type ViewerRun, type WorkflowScope,
} from "../src/index";
import { step } from "./graph.test";

function binding(source: string, source_port: string, target_port: string, source_kind: InterfaceBinding["source_kind"] = "output"): InterfaceBinding {
  return { source, source_port, target_port, source_kind };
}
function scope(id: string, changes: Partial<WorkflowScope> = {}): WorkflowScope {
  return { id, parent: null, kind: "workflow", path: id, step: id, take: [1], title: id, source: "workflows/inner.yaml",
    ports: { inputs: { image: { type: "file", kind: "image" }, count: { type: "integer", default: 2 } }, outputs: ["hero", "alternate"] },
    input_bindings: [], output_bindings: [], nodes: [], pending: [], ...changes };
}
export function nestedPlan(): GraphDocument {
  const a = "scope:batch.item#1", b = "scope:batch.item#2", nested = "scope:batch.item.child#1.1";
  return {
    kind: "fx-graph-v1", workflow: { id: "nested", title: "Nested workflow" }, pending: [],
    estimate: { low_usd: 0, high_usd: 0, ceiling_usd: null },
    types: {
      source: { identity: "fx/source@1.0", ports: { inputs: {}, outputs: { image: "image" }, params: {} } },
      change: { identity: "fx/change@1.0", ports: { inputs: { image: "image" }, outputs: { image: "image" }, params: { amount: { type: "integer" } } } },
      sink: { identity: "fx/sink@1.0", ports: { inputs: { image: "image", mask: "image" }, outputs: {}, params: {} } },
    },
    instances: [
      step("seed#1", { uses: "source", bindings: [], interface_bindings: [] }),
      step("a.paint#1", { uses: "change", reads: ["seed#1"], needs: ["seed#1"], bindings: [binding("seed#1", "image", "image", "output") as any],
        interface_bindings: [binding(a, "image", "image", "scope_input"), binding(a, "count", "amount", "scope_input")] }),
      step("b.paint#1", { uses: "change", reads: ["seed#1"], bindings: [binding("seed#1", "image", "image") as any], interface_bindings: [binding(b, "image", "image", "scope_input")] }),
      step("deep.paint#1", { uses: "change", reads: ["seed#1"], bindings: [binding("seed#1", "image", "image") as any], interface_bindings: [binding(nested, "image", "image", "scope_input")] }),
      step("save#1", { uses: "sink", reads: ["a.paint#1"], bindings: [binding("a.paint#1", "image", "image") as any, binding("a.paint#1", "image", "mask") as any],
        interface_bindings: [binding(a, "hero", "image", "scope_output"), binding(a, "alternate", "mask", "scope_output")] }),
    ],
    scopes: [
      scope("scope:batch#1", { kind: "group", title: "Batch", source: null, ports: { inputs: {}, outputs: [] } }),
      scope(a, { parent: "scope:batch#1", title: "Item one", input_bindings: [binding("seed#1", "image", "image")],
        output_bindings: [binding("a.paint#1", "image", "hero"), binding("a.paint#1", "image", "alternate")] }),
      scope("scope:a.inline#1", { parent: a, kind: "group", title: "Preparation", source: null, ports: { inputs: {}, outputs: [] }, nodes: ["a.paint#1"] }),
      scope(b, { parent: "scope:batch#1", title: "Item two", take: [2], nodes: ["b.paint#1"], input_bindings: [binding("seed#1", "image", "image")], output_bindings: [binding("b.paint#1", "image", "hero")] }),
      scope(nested, { parent: a, title: "Nested child", take: [1, 1], nodes: ["deep.paint#1"], input_bindings: [binding(a, "image", "image", "scope_input")], output_bindings: [binding("deep.paint#1", "image", "hero")] }),
    ],
  };
}
function run(plan: GraphDocument): ViewerRun {
  return { kind: "fx-viewer-run-v1", workflow: plan.workflow, run_name: "recorded", state: "failed", stand_in: false,
    charged_usd: 0, estimate: null, inputs: {}, outputs: {}, artifacts: [], warnings: [], scopes: plan.scopes,
    nodes: plan.instances.map((node) => ({ id: node.id, path: node.path, title: node.path, uses: node.uses,
      state: "succeeded", reads: node.reads, with: {}, outputs: {}, cache: null, error: null, duration_ms: null,
      ports: plan.types?.[node.uses].ports, bindings: node.bindings, interface_bindings: node.interface_bindings, needs: node.needs, judges: node.judges })) };
}

describe("scope contract", () => {
  test("accepts additive plan/run metadata and rejects malformed or cyclic membership", () => {
    const plan = nestedPlan();
    plan.scopes![0].take = [];
    expect(parseGraph(plan)).toEqual(plan);
    expect(parseViewerRun(run(plan)).scopes).toEqual(plan.scopes);
    const cycle = structuredClone(plan.scopes!); cycle[0].parent = cycle[1].id;
    expect(isWorkflowScopes(cycle)).toBeFalse();
    const duplicate = structuredClone(plan.scopes!); duplicate[0].nodes = ["a.paint#1"];
    expect(isWorkflowScopes(duplicate)).toBeFalse();
    const unknownParent = structuredClone(plan.scopes!); unknownParent[0].parent = "missing";
    expect(isWorkflowScopes(unknownParent)).toBeFalse();
    expect(isInterfaceBindings([binding("scope:a#1", "hero", "image", "scope_output")])).toBeTrue();
    expect(isInterfaceBindings([binding("leaf#1", "", "amount", "fact")])).toBeTrue();
    expect(isInterfaceBindings([binding("scope:a#1", "", "image", "scope_input")])).toBeFalse();
    expect(() => parseGraph({ ...plan, scopes: cycle })).toThrow("does not match");
    expect(() => parseViewerRun({ ...run(plan), scopes: null })).toThrow("malformed");
  });
});

describe("hierarchical graph projection", () => {
  test("collapses imported occurrences inside an expanded group and preserves exact public aliases", () => {
    const plan = nestedPlan();
    const before = JSON.stringify(plan);
    const graph = canvasGraph(plan);
    expect(graph.nodes.map((node) => node.id)).toEqual(["instance:seed#1", "instance:save#1", "scope:batch.item#1", "scope:batch.item#2"]);
    expect(graph.frames).toEqual([{ id: "scope:batch#1", title: "Batch", nodes: ["scope:batch.item#1", "scope:batch.item#2"], parent: null, address: "scope:batch#1", take: [1], key: "scope:batch#1", state: "planned" }]);
    const card = graph.nodes.find((node) => node.id === "scope:batch.item#1")!;
    expect(card).toMatchObject({ kind: "workflow", scope_id: card.id, child_count: 2, failure_count: 0, state: "planned" });
    expect(card.ports?.outputs).toEqual([{ name: "alternate", type: "unknown", kind: "value" }, { name: "hero", type: "unknown", kind: "value" }]);
    const aliases = graph.edges.filter((edge) => edge.target === "instance:save#1");
    expect(aliases.map((edge) => [edge.source, edge.source_port, edge.target_port])).toEqual([
      [card.id, "hero", "image"], [card.id, "alternate", "mask"],
    ]);
    expect(graph.edges.filter((edge) => edge.source === "instance:seed#1" && edge.target === card.id).map((edge) => edge.kind).sort()).toEqual(["control", "data"]);
    expect(new Set(graph.edges.map((edge) => edge.id)).size).toBe(graph.edges.length);
    expect(JSON.stringify(plan)).toBe(before);
    expect(flatCanvasGraph(plan).nodes).toHaveLength(5);
  });

  test("opens children with typed inputs, unknown output aliases, nested cards and direct group membership", () => {
    const graph = canvasGraph(nestedPlan(), "scope:batch.item#1");
    expect(graph.nodes.map((node) => node.id)).toEqual([
      "boundary:input:scope:batch.item#1", "instance:a.paint#1", "scope:batch.item.child#1.1", "boundary:output:scope:batch.item#1",
    ]);
    expect(graph.frames).toEqual([{ id: "scope:a.inline#1", title: "Preparation", nodes: ["instance:a.paint#1"], parent: null, address: "scope:a.inline#1", take: [1], key: "scope:a.inline#1", state: "planned" }]);
    expect(graph.nodes[0].ports?.outputs).toEqual([{ name: "count", type: "integer", kind: "parameter" }, { name: "image", type: "image", kind: "artifact" }]);
    expect(graph.nodes[1].ports?.inputs.some((port) => port.name === "amount" && port.kind === "parameter")).toBeTrue();
    expect(graph.edges.filter((edge) => edge.target === "instance:a.paint#1").map((edge) => [edge.source_port, edge.target_port])).toEqual([["image", "image"], ["count", "amount"]]);
    expect(graph.edges.filter((edge) => edge.target === "boundary:output:scope:batch.item#1").map((edge) => edge.target_port)).toEqual(["hero", "alternate"]);
    expect(graph.edges.some((edge) => edge.target === "scope:batch.item.child#1.1" && edge.source === graph.nodes[0].id)).toBeTrue();
    const nested = canvasGraph(nestedPlan(), "scope:batch.item.child#1.1");
    expect(nested.nodes.filter((node) => !node.kind).map((node) => node.source_id)).toEqual(["deep.paint#1"]);
  });

  test("each hierarchy level lays out finite geometry with exact boundary sockets", () => {
    const plan = nestedPlan();
    for (const scope of [null, ...plan.scopes!.filter((scope) => scope.kind === "workflow").map((scope) => scope.id)]) {
      const graph = canvasGraph(plan, scope);
      const layout = layoutGraph(graph);
      expect(layout).toEqual(layoutGraph(graph));
      expect(layout.nodes.every((node) => [node.x, node.y, node.width, node.height].every(Number.isFinite))).toBeTrue();
      for (const edge of layout.edges.filter((edge) => edge.kind === "data")) {
        const source = layout.nodes.find((node) => node.id === edge.source)!;
        const target = layout.nodes.find((node) => node.id === edge.target)!;
        expect(edge.points[0]).toEqual(canvasPortAnchor(source, "output", edge.source_port!, edge.source_kind === "fact" ? "fact" : undefined));
        expect(edge.points.at(-1)).toEqual(canvasPortAnchor(target, "input", edge.target_port!));
      }
    }
  });

  test("shows explicit pending membership and fact/passthrough boundary outputs without fabricated types", () => {
    const plan = nestedPlan();
    const scope = plan.scopes!.find((item) => item.id === "scope:batch.item#1")!;
    scope.pending = ["a.later"];
    scope.ports.outputs.push("count", "fact");
    scope.output_bindings.push(binding(scope.id, "count", "count", "scope_input"), binding("a.paint#1", "", "fact", "fact"));
    plan.pending.push({ path: "a.later", max: 3, phase: 1, high_usd: 0 });
    expect(canvasGraph(plan).nodes.find((node) => node.id === scope.id)).toMatchObject({ state: "unexpanded", child_count: 3 });
    const opened = canvasGraph(plan, scope.id);
    expect(opened.nodes.some((node) => node.id === "pending:a.later")).toBeTrue();
    expect(opened.edges.some((edge) => edge.source_kind === "fact" && edge.source_port === "" && edge.target_port === "fact")).toBeTrue();
    expect(opened.edges.some((edge) => edge.source.startsWith("boundary:input:") && edge.target_port === "count")).toBeTrue();
  });

  test("legacy remains flat and missing interface aliases only preserve known dependencies", () => {
    const plan = nestedPlan();
    const legacy = structuredClone(plan); delete legacy.scopes;
    expect(canvasGraph(legacy)).toEqual(flatCanvasGraph(legacy));
    expect(canvasGraph(legacy).nodes.some((node) => node.kind === "workflow")).toBeFalse();
    plan.instances.forEach((node) => { delete node.interface_bindings; });
    const edges = canvasGraph(plan).edges.filter((edge) => edge.target === "instance:save#1");
    expect(edges).toHaveLength(2);
    expect(edges.every((edge) => edge.kind === "dependency" && edge.source_port === undefined)).toBeTrue();
  });

  test("empty interface attribution retains known flattened dependencies without guessing aliases", () => {
    const plan = nestedPlan();
    const target = plan.instances.find((node) => node.id === "save#1")!;
    target.interface_bindings = [];
    let edges = canvasGraph(plan).edges.filter((edge) => edge.target === "instance:save#1");
    expect(edges).toHaveLength(2);
    expect(edges.every((edge) => edge.source === "scope:batch.item#1" && edge.kind === "dependency" && edge.source_port === undefined)).toBeTrue();
    // Exact attribution replaces the fallback when it is actually displayed.
    target.interface_bindings = [binding("scope:batch.item#1", "hero", "image", "scope_output")];
    edges = canvasGraph(plan).edges.filter((edge) => edge.target === "instance:save#1");
    expect(edges).toHaveLength(1);
    expect(edges[0]).toMatchObject({ kind: "data", source_port: "hero", target_port: "image" });
  });

  test("empty inline groups do not create phantom frames while enclosing groups remain", () => {
    const plan = nestedPlan();
    plan.scopes!.push(scope("scope:empty#", { kind: "group", title: "Empty", source: null, take: [], ports: { inputs: {}, outputs: [] } }));
    plan.scopes!.push(scope("scope:enclosing#", { kind: "group", title: "Enclosing", source: null, take: [], ports: { inputs: {}, outputs: [] } }));
    plan.scopes![0].parent = "scope:enclosing#";
    const frames = canvasGraph(plan).frames!;
    expect(frames.map((frame) => frame.id)).toEqual(["scope:batch#1", "scope:enclosing#"]);
    expect(frames[0].parent).toBe("scope:enclosing#");
    expect(frames[1].nodes).toEqual([]);
  });

  test("summarizes recorded descendant failures without declaring active or missing work successful", () => {
    const view = run(nestedPlan());
    const failed = view.nodes.find((node) => node.id === "deep.paint#1")!;
    failed.state = "failed"; failed.error = "The transform rejected its input.";
    let card = canvasGraph(view).nodes.find((node) => node.id === "scope:batch.item#1")!;
    expect(card).toMatchObject({ state: "failed", failure_count: 1, child_count: 2, errors: [{ id: failed.id, title: failed.title, message: failed.error }] });
    view.nodes.find((node) => node.id === "a.paint#1")!.state = "running";
    card = canvasGraph(view).nodes.find((node) => node.id === card.id)!;
    expect(card).toMatchObject({ state: "running", failure_count: 1 });
    expect(canvasGraph(view).nodes.find((node) => node.id === "scope:batch.item#2")!.state).toBe("succeeded");
    const skipped = view.nodes.find((node) => node.id === "b.paint#1")!;
    skipped.state = "skipped"; skipped.error = "Condition was false.";
    expect(canvasGraph(view).nodes.find((node) => node.id === "scope:batch.item#2")).toMatchObject({ state: "skipped", failure_count: 0 });
    expect(canvasGraph(view).nodes.find((node) => node.id === "scope:batch.item#2")!.errors).toBeUndefined();
  });

  test("recorded run pending paths prevent premature success and remain inside their inline group", () => {
    const view = run(nestedPlan());
    view.state = "running";
    view.scopes!.find((scope) => scope.id === "scope:a.inline#1")!.pending = ["a.inline.waiting_items"];
    const card = canvasGraph(view).nodes.find((node) => node.id === "scope:batch.item#1")!;
    expect(card).toMatchObject({ state: "pending", child_count: 3, failure_count: 0 });
    const opened = canvasGraph(view, card.id);
    const pending = opened.nodes.find((node) => node.pending)!;
    expect(pending).toEqual({ id: "pending:a.inline.waiting_items", source_id: "a.inline.waiting_items", title: "a.inline.waiting_items", subtitle: "Waiting for repeat expansion", state: "unexpanded", pending: true });
    expect(opened.frames!.find((frame) => frame.id === "scope:a.inline#1")!.nodes).toContain(pending.id);
    expect(opened.edges.some((edge) => edge.source === pending.id || edge.target === pending.id)).toBeFalse();
    expect(flatCanvasGraph(view).nodes).toHaveLength(view.nodes.length);
  });
});

test("viewer navigation skips inline frames and preserves the selection on refresh and return", async () => {
  const plan = nestedPlan();
  expect(navigationScopes(plan.scopes).map((scope) => [scope.id, scope.parent])).toEqual([
    ["scope:batch.item#1", null], ["scope:batch.item#2", null], ["scope:batch.item.child#1.1", "scope:batch.item#1"],
  ]);
  const controller = new ViewerController(async () => plan);
  await controller.refresh();
  controller.select("scope:batch.item#1");
  controller.openScope("scope:batch.item#1");
  expect(controller.getSnapshot().scopeNode).toMatchObject({ id: "scope:batch.item#1", title: "Item one", child_count: 2 });
  controller.select("instance:a.paint#1");
  controller.openScope("scope:batch.item.child#1.1");
  controller.select("instance:deep.paint#1");
  await controller.refresh();
  expect(controller.getSnapshot()).toMatchObject({ scope: "scope:batch.item.child#1.1", selected: "instance:deep.paint#1" });
  expect(controller.getSnapshot().breadcrumbs.map((crumb) => crumb.id)).toEqual([null, "scope:batch.item#1", "scope:batch.item.child#1.1"]);
  controller.back();
  expect(controller.getSnapshot().selected).toBe("instance:a.paint#1");
  controller.goToScope(null);
  expect(controller.getSnapshot().selected).toBe("scope:batch.item#1");
  expect(controller.getSnapshot().scopeNode).toBeNull();
  controller.openScope("scope:batch#1");
  expect(controller.getSnapshot().scope).toBeNull();
});

test("reference navigation uses explicit visible aliases and never guesses from path prefixes", async () => {
  const controller = new ViewerController(async () => nestedPlan());
  await controller.refresh();
  expect(controller.referenceTarget("scope:batch.item#1", "scope_output")).toBe("scope:batch.item#1");
  expect(controller.referenceTarget("deep.paint#1")).toBe("scope:batch.item#1");
  expect(controller.referenceTarget("deep.paint")).toBeNull();
  expect(controller.referenceTarget("scope:batch.item#1", "scope_input")).toBeNull();
  controller.selectReference("deep.paint#1");
  expect(controller.getSnapshot().selected).toBe("scope:batch.item#1");
  controller.openScope("scope:batch.item#1");
  expect(controller.referenceTarget("scope:batch.item#1", "scope_input")).toBe("boundary:input:scope:batch.item#1");
  expect(controller.referenceTarget("scope:batch.item.child#1.1", "scope_output")).toBe("scope:batch.item.child#1.1");
  expect(controller.referenceTarget("a.paint#1", "fact")).toBe("instance:a.paint#1");
  expect(controller.referenceTarget("seed#1")).toBeNull();
  controller.selectReference("scope:batch.item#1", "scope_input");
  controller.selectReference("seed#1");
  expect(controller.getSnapshot().selected).toBe("boundary:input:scope:batch.item#1");
});
