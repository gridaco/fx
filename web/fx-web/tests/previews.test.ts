import { describe, expect, test } from "bun:test";
import { canvasGraph, type Artifact, type ViewerRun } from "../src/index";
import { graph } from "./graph.test";

const sourceDigest = "a".repeat(64);
const outputDigest = "b".repeat(64);
const otherDigest = "c".repeat(64);
const file = (digest: string) => ({ file: { digest, kind: "image/png", name: "image.png", size: 100 } });
const artifact = (digest: string, changes: Partial<Artifact> = {}): Artifact => ({
  digest, kind: "image/png", name: "image.png", size: 100,
  available: true, url: `/api/artifacts/${digest}`, ...changes,
});

function run(): ViewerRun {
  return {
    kind: "fx-viewer-run-v1", workflow: { id: "sample", title: "Sample" }, run_name: "first",
    state: "succeeded", stand_in: false, charged_usd: 0, estimate: null,
    inputs: { source: file(sourceDigest) }, outputs: { image: file(outputDigest) }, warnings: [],
    nodes: [{
      id: "change#1", path: "change", title: "Change", uses: "./change.py#transform", state: "succeeded",
      reads: [], with: { source: file(sourceDigest) }, outputs: { image: file(outputDigest) },
      cache: null, error: null, duration_ms: 5,
    }],
    artifacts: [artifact(sourceDigest), artifact(outputDigest)],
  };
}

describe("recorded canvas image previews", () => {
  test("plans have no previews, including cached or planning-time results", () => {
    const plan = graph();
    plan.instances[0].state = "done";
    plan.instances[0].with = { source: file(sourceDigest) };
    plan.instances[0].view = true;
    expect(canvasGraph(plan).nodes.every((node) => node.preview === undefined)).toBeTrue();
  });

  test("uses an output image without mutating recorded values or consulting inputs", () => {
    const recorded = run();
    const before = JSON.stringify(recorded);
    expect(canvasGraph(recorded).nodes[0].preview).toEqual({
      kind: "image", digest: outputDigest, url: `/api/artifacts/${outputDigest}`,
      label: "image: image.png", count: 1,
    });
    expect(JSON.stringify(recorded)).toBe(before);
    recorded.nodes[0].outputs = {};
    expect(canvasGraph(recorded).nodes[0].preview).toBeUndefined();
  });

  test("finds nested file references and counts distinct available image outputs", () => {
    const recorded = run();
    recorded.nodes[0].outputs = {
      gallery: { collection: [["first", { list: [file(otherDigest), file(outputDigest)] }]] },
      duplicate: file(otherDigest),
    };
    recorded.artifacts.push(artifact(otherDigest, { name: "second.png" }));
    expect(canvasGraph(recorded).nodes[0].preview).toEqual({
      kind: "image", digest: otherDigest, url: `/api/artifacts/${otherDigest}`,
      label: "gallery: second.png", count: 2,
    });
  });

  test("skips unavailable images and chooses the next available output", () => {
    const recorded = run();
    recorded.artifacts[1].available = false;
    recorded.nodes[0].outputs.more = file(otherDigest);
    recorded.artifacts.push(artifact(otherDigest));
    expect(canvasGraph(recorded).nodes[0].preview?.digest).toBe(otherDigest);
    expect(canvasGraph(recorded).nodes[0].preview?.count).toBe(1);
    recorded.artifacts[2].url = null;
    expect(canvasGraph(recorded).nodes[0].preview).toBeUndefined();
  });

  test("does not embed active documents, unverified references, or foreign URLs", () => {
    for (const changes of [
      { kind: "image/svg+xml" }, { kind: "text/html" },
      { url: "https://example.invalid/image.png" }, { url: "/unverified/image.png" },
    ]) {
      const recorded = run();
      recorded.artifacts[1] = artifact(outputDigest, changes);
      expect(canvasGraph(recorded).nodes[0].preview).toBeUndefined();
    }
    const recorded = run();
    recorded.artifacts = [];
    expect(canvasGraph(recorded).nodes[0].preview).toBeUndefined();
  });

  test("ordinary JSON that resembles a file reference stays opaque", () => {
    const recorded = run();
    recorded.nodes[0].outputs = {
      metadata: { value: { file: { digest: sourceDigest, kind: "image/png", size: 100 } } },
      paths: { value: [file(outputDigest)] },
      ordinary: { nested: file(outputDigest) },
      plain: { file: sourceDigest },
    };
    expect(canvasGraph(recorded).nodes[0].preview).toBeUndefined();
  });
});
