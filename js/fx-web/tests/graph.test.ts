import { describe, expect, test } from "bun:test";
import { canvasGraph, layoutGraph, parseGraph, parseViewerView, type GraphDocument, type GraphInstance } from "../src/index";

export function step(id: string, changes: Partial<GraphInstance> = {}): GraphInstance {
  return {
    id, path: id.split("#")[0], step: id.split("#")[0], take: [1], uses: "./nodes.py#transform",
    type: "nodes.py#transform@1", with: {}, routes: {}, state: "planned", identity: null,
    phase: 1, key: null, judges: null, judged_by: [], waiting_on: [], needs: [], reads: [],
    price: { low_usd: 0, high_usd: 0 }, ...changes,
  };
}

export function graph(): GraphDocument {
  return {
    kind: "fx-graph-v1", workflow: { id: "sample", title: "Sample" },
    instances: [step("read#1"), step("change#1", { reads: ["read#1"], needs: ["read#1"], waiting_on: ["read#1"], with: { source: { pending: ["read#1"] } } })],
    pending: [{ path: "items", max: 4, phase: 2, high_usd: 0 }],
    estimate: { low_usd: 0, high_usd: 0, ceiling_usd: null }, problems: [],
  };
}

describe("plan boundary and graph projection", () => {
  test("the view discriminator reads a static graph without inventing run fields", () => {
    const input = graph();
    expect(parseViewerView(input)).toEqual(input);
    expect("charged_usd" in parseViewerView(input)).toBeFalse();
  });
  test("preserves every static plan state", () => {
    const input = graph();
    input.instances = ["planned", "maybe", "absent", "blocked", "failed", "done"].map((state, index) => step(`item${index}#1`, { state: state as GraphInstance["state"] }));
    expect(canvasGraph(parseGraph(input)).nodes.filter((node) => !node.pending).map((node) => node.state)).toEqual(input.instances.map((item) => item.state));
  });
  test("refuses missing fields, unknown states, invalid pending markers, and duplicate IDs", () => {
    const noCeiling = graph() as unknown as Record<string, any>;
    delete noCeiling.estimate.ceiling_usd;
    expect(() => parseGraph(noCeiling)).toThrow("does not match");
    const invalidState = graph();
    invalidState.instances[0].state = "succeeded" as GraphInstance["state"];
    expect(() => parseGraph(invalidState)).toThrow("does not match");
    const invalidPending = graph();
    invalidPending.instances[0].with = { source: { pending: [3] } };
    expect(() => parseGraph(invalidPending)).toThrow("does not match");
    const duplicate = graph();
    duplicate.instances.push(duplicate.instances[0]);
    expect(() => parseGraph(duplicate)).toThrow("does not match");
  });
  test("parameter names are not interpreted as value markers", () => {
    const input = graph();
    input.instances[0].with = { pending: [1, 2, 3] };
    expect(parseGraph(input)).toEqual(input);
  });
  test("merges declared edge roles and leaves unexpanded repeats as placeholders", () => {
    const projected = canvasGraph(graph());
    expect(projected.edges).toEqual([{ id: JSON.stringify(["control", "instance:read#1", "instance:change#1"]), source: "instance:read#1", target: "instance:change#1", kind: "control", kinds: ["needs", "reads", "waiting"] }]);
    expect(projected.nodes.at(-1)).toMatchObject({ id: "pending:items", source_id: "items", state: "unexpanded", pending: true });
    expect(projected.edges.some((edge) => edge.source === "pending:items" || edge.target === "pending:items")).toBeFalse();
  });
  test("does not guess a source for ambiguous takes or fabricate missing nodes", () => {
    const input = graph();
    input.instances = [step("read#1"), step("read#2", { take: [2] }), step("change#1", { reads: ["read", "unknown#1"] })];
    expect(canvasGraph(input).edges).toEqual([]);
  });
  test("layout is deterministic and never changes the workflow", () => {
    const input = graph();
    const before = JSON.stringify(input);
    const projected = canvasGraph(input);
    expect(layoutGraph(projected)).toEqual(layoutGraph(projected));
    expect(JSON.stringify(input)).toBe(before);
    const placed = layoutGraph(projected);
    expect(placed.nodes.find((node) => node.id === "instance:change#1")!.x).toBeGreaterThan(placed.nodes.find((node) => node.id === "instance:read#1")!.x);
  });
});
