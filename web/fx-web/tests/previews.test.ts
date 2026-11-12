import { describe, expect, test } from "bun:test";
import { canvasGraph, cardPreview, parseGraph, parseViewerRun, type Artifact, type ViewerRun } from "../src/index";
import { resumeAt } from "../src/video";
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

describe("recorded canvas previews", () => {
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

  test("a step without an image offers its first native-video candidate, including Matroska", () => {
    const recorded = run();
    const video = (digest: string, kind: string, name: string) => ({ file: { digest, kind, name, size: 100 } });
    recorded.nodes[0].outputs = { raw: video(sourceDigest, "video/x-matroska", "raw.mkv"), clips: { list: [video(otherDigest, "video/webm", "a.webm"), video(outputDigest, "video/mp4", "b.mp4")] } };
    recorded.artifacts = [
      artifact(sourceDigest, { kind: "video/x-matroska", name: "raw.mkv" }),
      artifact(otherDigest, { kind: "video/webm", name: "a.webm" }),
      artifact(outputDigest, { kind: "video/mp4", name: "b.mp4" }),
    ];
    expect(canvasGraph(recorded).nodes[0].preview).toEqual({
      kind: "video", digest: sourceDigest, url: `/api/artifacts/${sourceDigest}`,
      label: "raw: raw.mkv", count: 3,
    });
    recorded.artifacts[0].available = false;
    expect(canvasGraph(recorded).nodes[0].preview).toMatchObject({ digest: otherDigest, count: 2 });
    recorded.artifacts[1].url = "https://example.invalid/a.webm";
    expect(canvasGraph(recorded).nodes[0].preview).toMatchObject({ digest: outputDigest, count: 1 });
  });

  test("accepts concrete video types without treating malformed kinds as native media", () => {
    for (const kind of ["video/quicktime", "video/x-acme+video", "video/mpeg"]) {
      const recorded = run();
      recorded.artifacts[1].kind = kind;
      expect(canvasGraph(recorded).nodes[0].preview?.kind).toBe("video");
    }
    for (const kind of ["video/", "video/*", "video/mp4[]", "video/mp4; codecs=avc1", "video/mp4\r\nx-acme: value"]) {
      const recorded = run();
      recorded.artifacts[1].kind = kind;
      expect(canvasGraph(recorded).nodes[0].preview).toBeUndefined();
    }
  });

  test("an image output previews its card before a video output does", () => {
    const recorded = run();
    recorded.nodes[0].outputs = { video: { file: { digest: otherDigest, kind: "video/mp4", name: "clip.mp4", size: 100 } }, image: file(outputDigest) };
    recorded.artifacts.push(artifact(otherDigest, { kind: "video/mp4", name: "clip.mp4" }));
    expect(canvasGraph(recorded).nodes[0].preview).toMatchObject({ kind: "image", digest: outputDigest, count: 1 });
  });

  test("a model card uses its own GLB output and recorded verification metadata, never its reference image", () => {
    const recorded = run();
    recorded.nodes[0].outputs = { model: { file: { digest: outputDigest, kind: "model/gltf-binary", name: "asset.glb", size: 300 } } };
    recorded.artifacts[1] = artifact(outputDigest, { kind: "model/gltf-binary", name: "asset.glb", size: 300 });
    const before = JSON.stringify(recorded);
    expect(canvasGraph(recorded).nodes[0].preview).toEqual({
      kind: "model", digest: outputDigest, name: "asset.glb", size: 300,
      url: `/api/artifacts/${outputDigest}`, label: "model: asset.glb", count: 1,
    });
    expect(JSON.stringify(recorded)).toBe(before);
    recorded.nodes[0].outputs = {};
    expect(canvasGraph(recorded).nodes[0].preview).toBeUndefined();
  });

  test("models follow available images and videos, preserving existing preview priority", () => {
    const recorded = run();
    recorded.nodes[0].outputs = { model: file(sourceDigest), clip: file(otherDigest), image: file(outputDigest) };
    recorded.artifacts[0] = artifact(sourceDigest, { kind: "model/gltf-binary", name: "asset.glb" });
    recorded.artifacts.push(artifact(otherDigest, { kind: "video/mp4", name: "clip.mp4" }));
    expect(canvasGraph(recorded).nodes[0].preview).toMatchObject({ kind: "image", digest: outputDigest });
    recorded.artifacts[1].available = false;
    expect(canvasGraph(recorded).nodes[0].preview).toMatchObject({ kind: "video", digest: otherDigest });
    recorded.artifacts[2].available = false;
    expect(canvasGraph(recorded).nodes[0].preview).toMatchObject({ kind: "model", digest: sourceDigest });
  });

  test("GLB collections count distinct available outputs and retain unavailable-source fallbacks", () => {
    const recorded = run();
    recorded.nodes[0].outputs = { models: { collection: [["first", { list: [file(otherDigest), file(outputDigest), file(otherDigest)] }]] } };
    recorded.artifacts[1] = artifact(outputDigest, { kind: "model/gltf-binary", name: "second.glb" });
    recorded.artifacts.push(artifact(otherDigest, { kind: "model/gltf-binary", name: "first.glb" }));
    expect(canvasGraph(recorded).nodes[0].preview).toMatchObject({ kind: "model", digest: otherDigest, count: 2 });
    recorded.artifacts[2].available = false;
    expect(canvasGraph(recorded).nodes[0].preview).toMatchObject({ kind: "model", digest: outputDigest, count: 1 });
    recorded.artifacts[1].available = false;
    expect(canvasGraph(recorded).nodes[0].preview).toMatchObject({ kind: "model", digest: otherDigest, url: null, count: 2, unavailable_reason: "unavailable" });
  });

  test("model previews require GLB inventory metadata and never authorize a foreign artifact URL", () => {
    for (const kind of ["model/fbx", "model/gltf+json", "model/*", "model/gltf-binary[]"]) {
      const recorded = run();
      recorded.artifacts[1].kind = kind;
      expect(canvasGraph(recorded).nodes[0].preview).toBeUndefined();
    }
    const recorded = run();
    recorded.artifacts[1] = artifact(outputDigest, { kind: "model/gltf-binary", name: "asset.glb", url: "https://acme.invalid/asset.glb" });
    expect(canvasGraph(recorded).nodes[0].preview).toMatchObject({ kind: "model", url: null, unavailable_reason: "unavailable" });
    for (const size of [undefined, -1, NaN, Infinity, 1.5]) {
      expect(cardPreview({ model: file(outputDigest) }, () => ({ media_type: "model/gltf-binary", name: "asset.glb", url: "/verified/model.glb", size }))).toBeUndefined();
    }
  });

  test("explicit video selection keeps its authored poster without selecting unrelated images", () => {
    const recorded = run();
    recorded.nodes[0].preview = { video: "clip", poster: "poster" };
    recorded.nodes[0].outputs = {
      unrelated: file(outputDigest), poster: file(otherDigest),
      clip: { file: { digest: sourceDigest, kind: "video/mp4", name: "clip.mp4", size: 100 } },
    };
    recorded.artifacts = [artifact(outputDigest), artifact(otherDigest, { name: "poster.png" }), artifact(sourceDigest, { kind: "video/mp4", name: "clip.mp4" })];
    const before = JSON.stringify(recorded);
    expect(canvasGraph(parseViewerRun(recorded)).nodes[0].preview).toEqual({
      kind: "video", digest: sourceDigest, url: `/api/artifacts/${sourceDigest}`, label: "clip: clip.mp4", count: 1,
      poster: { digest: otherDigest, url: `/api/artifacts/${otherDigest}`, label: "poster: poster.png" },
    });
    expect(JSON.stringify(recorded)).toBe(before);
    recorded.artifacts[2].available = false;
    expect(canvasGraph(recorded).nodes[0].preview).toMatchObject({ kind: "video", url: null, unavailable_reason: "unavailable", poster: { digest: otherDigest } });
  });

  test("unavailable, unsafe, non-image, and non-scalar posters fall back to the selected video", () => {
    const recorded = run();
    recorded.nodes[0].preview = { video: "clip", poster: "image" };
    recorded.nodes[0].outputs.clip = { file: { digest: otherDigest, kind: "video/mp4", name: "clip.mp4", size: 100 } };
    recorded.artifacts.push(artifact(otherDigest, { kind: "video/mp4", name: "clip.mp4" }));
    for (const changes of [{ available: false }, { url: "https://example.invalid/poster.png" }, { kind: "image/svg+xml" }]) {
      const changed = structuredClone(recorded);
      Object.assign(changed.artifacts[1], changes);
      const preview = canvasGraph(changed).nodes[0].preview;
      expect(preview).toMatchObject({ kind: "video", digest: otherDigest });
      expect(preview && "poster" in preview).toBeFalse();
    }
    for (const poster of [{ list: [file(outputDigest)] }, { value: file(outputDigest) }, { withheld: true }]) {
      recorded.nodes[0].outputs.image = poster;
      expect(canvasGraph(recorded).nodes[0].preview).toEqual({ kind: "video", digest: otherDigest, url: `/api/artifacts/${otherDigest}`, label: "clip: clip.mp4", count: 1 });
    }
    recorded.nodes[0].outputs.clip = { list: [file(otherDigest)] };
    expect(canvasGraph(recorded).nodes[0].preview).toBeUndefined();
  });

  test("plan and run readers accept own-output preview names and reject expressions or foreign metadata", () => {
    const descriptor = { video: "clip", poster: "poster_image" };
    const plan = graph();
    plan.instances[0].preview = descriptor;
    plan.steps = { prepare: { preview: descriptor } };
    expect(parseGraph(plan).instances[0].preview).toEqual(descriptor);
    expect(canvasGraph(plan).nodes[0].preview).toBeUndefined();
    const longPort = `clip_${"long_".repeat(20)}`;
    expect(parseGraph({ ...plan, instances: [{ ...plan.instances[0], preview: { video: longPort } }] }).instances[0].preview?.video).toBe(longPort);
    const longRecorded = run();
    longRecorded.nodes[0].preview = { video: longPort };
    expect(parseViewerRun(longRecorded).nodes[0].preview?.video).toBe(longPort);
    for (const preview of [{}, { video: "" }, { video: "steps.other.outputs.clip" }, { video: "${{ outputs.clip }}" }, { video: "clip", poster: "https://example.invalid/poster.png" }, { video: "clip", poster: null }, { video: "clip", extra: true }]) {
      expect(() => parseGraph({ ...plan, instances: [{ ...plan.instances[0], preview }] })).toThrow();
      expect(() => parseGraph({ ...plan, steps: { prepare: { preview } } })).toThrow();
      const recorded = run();
      expect(() => parseViewerRun({ ...recorded, nodes: [{ ...recorded.nodes[0], preview }] })).toThrow();
    }
  });

  test("an optional video with no recorded file does not turn its poster into a video source", () => {
    const recorded = run();
    recorded.nodes[0].preview = { video: "clip", poster: "image" };
    recorded.nodes[0].outputs.clip = { value: null };
    expect(canvasGraph(recorded).nodes[0].preview).toBeUndefined();
  });

  test("a video continues where it stopped, and starts over once it has played through", () => {
    expect(resumeAt(3.2, 8)).toBe(3.2);
    expect([resumeAt(0, 8), resumeAt(7.9, 8), resumeAt(8, 8), resumeAt(NaN, 8)]).toEqual([null, null, null, null]);
    // A stream of unknown length always continues.
    expect(resumeAt(40, Infinity)).toBe(40);
  });
});
