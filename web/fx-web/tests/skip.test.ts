import { describe, expect, test } from "bun:test";
import { canvasGraph, isSkipReason, parseRunExport, parseViewerRun, skipExplanation, skipPreviewLines, skipSourceScopes, type RunNode, type ViewerRun, type WorkflowScope } from "../src/index";
import { readFileSync } from "node:fs";

function node(id: string, take = 1): RunNode {
  return { id, path: id, take: [take], title: "Draw a candidate", uses: null, state: "succeeded", reads: [], with: {}, outputs: {}, cache: null, duration_ms: 0, error: null };
}
function run(): ViewerRun {
  return { kind: "fx-viewer-run-v1", workflow: { id: "acme", title: "Example" }, run_name: "saved", state: "succeeded", stand_in: false,
    charged_usd: 0, estimate: null, inputs: {}, outputs: {}, artifacts: [], warnings: [], nodes: [node("draw#2", 2),
      { ...node("draw#3", 3), state: "skipped", duration_ms: null, skip_reason: { code: "earlier_take_accepted", sources: ["draw#2"] } }] };
}

describe("recorded skip explanations", () => {
  test("unused pet-style takes cite the recorded accepted take without parsing instance IDs", () => {
    const earlier: RunNode = { ...node("opaque-accepted-id", 2), take: [4, 2] };
    const explanation = skipExplanation({ code: "earlier_take_accepted", sources: [earlier.id] }, [earlier]);
    expect(explanation.message).toBe("This take wasn’t needed because take 2 was accepted.");
    expect(explanation.related).toEqual([{ id: earlier.id, label: "Draw a candidate · take 4.2" }]);
    delete earlier.take;
    expect(skipExplanation({ code: "earlier_take_accepted", sources: [earlier.id] }, [earlier]).message)
      .toBe("This take wasn’t needed because an earlier take was accepted.");
  });

  test("known causes express their different meanings without turning every skip into nonexecution", () => {
    const messages = {
      condition_false: "The condition for running this step was not met.",
      earlier_take_settled: "An earlier take already settled this sequence.",
      upstream_failed: "A required upstream step failed.",
      upstream_absent: "A required upstream result was unavailable.",
      rejected: "The candidate was rejected and the workflow skips this path.",
      assertion_skip: "A workflow check failed and was configured to skip this step.",
      no_candidate: "None of the supplied candidates was available.",
    };
    for (const [code, message] of Object.entries(messages)) expect(skipExplanation({ code, sources: [] }, []).message).toBe(message);
  });

  test("missing, unknown and unverifiable causes do not become guessed explanations or links", () => {
    const earlier = { ...node("earlier#1"), error: "an earlier take was accepted" };
    expect(skipExplanation(undefined, [earlier])).toEqual({ message: "Reason not recorded.", related: [] });
    for (const reason of [{ code: "future_reason", sources: [earlier.id] }, { code: "earlier_take_accepted", sources: ["missing#1"] }])
      expect(skipExplanation(reason, [earlier])).toEqual({ message: "Skip reason unavailable in this viewer.", related: [] });
  });

  test("transport validates structured facts and accepts future codes without evaluating them", () => {
    expect(parseViewerRun(run()).nodes[1].skip_reason?.code).toBe("earlier_take_accepted");
    expect(isSkipReason({ code: "future_cause", sources: [] })).toBeTrue();
    for (const reason of [null, "because", { code: "Condition false", sources: [] }, { code: "condition_false" }, { code: "condition_false", sources: ["a", "a"] },
      { code: "condition_false", sources: [1] }, { code: "condition_false", sources: [], message: "invented explanation" }]) {
      const value = run() as any;
      value.nodes[1].skip_reason = reason;
      expect(() => parseViewerRun(value)).toThrow("malformed");
    }
    const value = run(); value.nodes[1].state = "succeeded";
    expect(() => parseViewerRun(value)).toThrow("malformed");
  });

  test("projection keeps its own available preview and attaches cause only to skipped cards", () => {
    const value = run(), digest = "a".repeat(64);
    value.nodes[1].outputs = { image: { file: { digest, kind: "image/png", name: "candidate.png", size: 1 } } };
    value.artifacts = [{ digest, kind: "image/png", name: "candidate.png", size: 1, available: true, url: `/api/artifacts/${digest}` }];
    const graph = canvasGraph(value);
    expect(graph.nodes[0].skip_explanation).toBeUndefined();
    expect(graph.nodes[1].preview?.digest).toBe(digest);
    expect(graph.nodes[1].skip_explanation?.message).toContain("take 2 was accepted");
    delete value.nodes[1].skip_reason;
    expect(canvasGraph(value).nodes[1].skip_explanation?.message).toBe("Reason not recorded.");
  });

  test("frozen exports preserve valid cause references and refuse dangling related nodes", () => {
    const manifest = JSON.parse(readFileSync(new URL("../../../spec/vectors/exports/minimal.json", import.meta.url), "utf8"));
    manifest.view.nodes[0].state = "skipped";
    manifest.view.nodes[0].skip_reason = { code: "condition_false", sources: [] };
    expect(parseRunExport(manifest).view.nodes[0].skip_reason?.code).toBe("condition_false");
    manifest.view.nodes[0].skip_reason.sources = ["missing#1"];
    expect(() => parseRunExport(manifest)).toThrow("skip reason names a missing node");
  });

  test("card text is bounded while related navigation follows recorded workflow membership", () => {
    const lines = skipPreviewLines("This take wasn’t needed because take 2 was accepted.");
    expect(lines.join(" ")).toBe("This take wasn’t needed because take 2 was accepted.");
    expect(lines.every((line) => [...line].length <= 43)).toBeTrue();
    expect(skipPreviewLines("a ".repeat(200))).toHaveLength(3);
    expect(skipPreviewLines("x".repeat(80))[0]).toHaveLength(43);
    const scope = (id: string, parent: string | null, kind: "group" | "workflow", nodes: string[]): WorkflowScope =>
      ({ id, parent, kind, nodes, pending: [], path: id, step: id, take: [1], title: id, source: null, ports: { inputs: {}, outputs: [] }, input_bindings: [], output_bindings: [] });
    const scopes = [scope("outer", null, "workflow", []), scope("group", "outer", "group", []), scope("inner", "group", "workflow", ["source#1"])];
    expect(skipSourceScopes("source#1", scopes)).toEqual(["outer", "inner"]);
    expect(skipSourceScopes("unrelated#1", scopes)).toEqual([]);
  });
});
