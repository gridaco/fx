import { describe, expect, test } from "bun:test";
import { artifactDigests, parseViewerRun } from "../src/index";

const digest = "a".repeat(64);

function response() {
  return {
    kind: "fx-viewer-run-v1",
    workflow: { id: "sample", title: "Sample" },
    run_name: "example-run",
    state: "succeeded",
    stand_in: true,
    charged_usd: null,
    estimate: null,
    inputs: {},
    outputs: {},
    nodes: [{
      id: "draw#1", path: "draw", title: "Draw", uses: null,
      state: "succeeded", reads: [], with: {}, outputs: {},
      cache: null, error: null, duration_ms: null,
    }],
    artifacts: [{
      digest, kind: "image/png", name: "sample.png", size: 12,
      available: true, url: `/api/artifacts/${digest}` as string | null,
    }],
    warnings: [],
  };
}

describe("viewer read boundary", () => {
  test("preserves unknown values and explicit unavailable artifacts", () => {
    const input = response();
    input.artifacts[0].available = false;
    input.artifacts[0].url = null;
    expect(parseViewerRun(input)).toEqual(input);
    expect(parseViewerRun(input).charged_usd).toBeNull();
  });

  test("refuses unsupported contracts before reading fields", () => {
    expect(() => parseViewerRun({ ...response(), kind: "fx-viewer-run-v2" })).toThrow("does not support");
    expect(() => parseViewerRun(null)).toThrow("does not support");
  });

  test("requires unknown values to be explicitly null", () => {
    for (const field of ["charged_usd", "estimate"] as const) {
      const input = response() as Record<string, unknown>;
      delete input[field];
      expect(() => parseViewerRun(input)).toThrow("incomplete or malformed");
    }
    for (const field of ["uses", "cache", "error", "duration_ms"] as const) {
      const input = response();
      delete (input.nodes[0] as Record<string, unknown>)[field];
      expect(() => parseViewerRun(input)).toThrow("incomplete or malformed");
    }
  });

  test("refuses external, active, traversal, and mismatched artifact URLs", () => {
    const urls = [
      "https://example.com/image.png", "//example.com/image.png", "javascript:alert(1)",
      "data:image/png;base64,AAAA", "/api/artifacts/../../file", "/api/artifacts/" + "b".repeat(64),
    ];
    for (const url of urls) {
      const input = response();
      input.artifacts[0].url = url;
      expect(() => parseViewerRun(input)).toThrow("incomplete or malformed");
    }
  });

  test("finds recorded and plain file references inside collections", () => {
    expect(artifactDigests({ collection: [
      ["first", { file: { digest, kind: "image/png", size: 12 } }],
      ["second", { file: digest }],
      ["ordinary", { value: { file: "example.png" } }],
    ] })).toEqual([digest]);
  });
});
