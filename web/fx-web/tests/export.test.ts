import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { ExportViewerController, canvasGraph, exportArtifactUrl, exportCanvasGraph, parseRunExport, parseViewerRun, readRunExport, type LoadedRunExport, type RunExport } from "../src/index";

function fixture(): RunExport { return JSON.parse(readFileSync(new URL("../../../spec/vectors/exports/minimal.json", import.meta.url), "utf8")); }
const imageDigest = "b".repeat(64);
function withImage(omitted = false): RunExport {
  const value = fixture();
  const artifact = { digest: imageDigest, media_type: "image/png", name: "preview.png", size: 20, path: omitted ? null : `artifacts/${imageDigest}.png`, omitted_reason: omitted ? "unplaced_input" : null };
  value.artifacts = [artifact];
  value.view.artifacts = [{ digest: imageDigest, kind: "image/png", name: "preview.png", size: 20, available: !omitted, url: null }];
  value.view.nodes[0].outputs = { image: { file: { digest: imageDigest, kind: "image/png", name: "preview.png", size: 20 } } };
  return value;
}
function loaded(manifest = fixture()): LoadedRunExport { return { manifest: parseRunExport(manifest), manifest_url: "https://assets.example.com/runs/example/manifest.json" }; }
function hash(bytes: Uint8Array) { return new Bun.CryptoHasher("sha256").update(bytes).digest("hex"); }
function nested(): RunExport {
  const value = withImage();
  value.view.nodes[0].step = "sub.prepare";
  value.view.nodes[0].ports = { inputs: {}, outputs: { image: "image" }, params: {} };
  value.view.scopes = [{ id: "scope:sub#1", parent: null, kind: "workflow", path: "sub", step: "sub", take: [1], title: "Imported workflow", source: null, ports: { inputs: {}, outputs: ["result"] }, input_bindings: [], output_bindings: [{ source: "prepare#1", source_port: "image", source_kind: "output", target_port: "result" }], nodes: ["prepare#1"], pending: [] }];
  value.layout.cells = { sub: { column: 0, row: 0, source: "automatic" }, "sub.prepare": { column: 0, row: 0, source: "automatic" } };
  value.layout.order.push("scope:sub#1");
  return value;
}

describe("portable inspection boundary", () => {
  test("reads the independent frozen v1 vector and permits unknown optional metadata", () => {
    const value = fixture();
    expect(parseRunExport(value)).toEqual(value);
    expect(parseRunExport({ ...value, optional_future_note: "ignored", producer: { ...value.producer, optional_future_note: true } }).kind).toBe("fx-run-export-v1");
    expect(() => parseRunExport({ ...value, kind: "fx-run-export-v2" })).toThrow("does not support");
    expect(() => parseRunExport({ ...value, source: { ...value.source, event_count: 0 } })).toThrow("metadata");
  });
  test("ignores additive fields inside frozen nested schemas without widening local validation", () => {
    const value = nested() as unknown as Record<string, any>;
    value.view.nodes[0].ports.optional_future_note = true;
    value.view.scopes[0].optional_future_note = true;
    value.view.scopes[0].ports.optional_future_note = true;
    value.view.scopes[0].output_bindings[0].optional_future_note = true;
    expect(parseRunExport(value).view.scopes).toHaveLength(1);
    expect(() => parseViewerRun(value.view)).toThrow("malformed");
  });
  test("included author values preserve scalar, null, array and ordinary object positions", () => {
    const value = fixture(); value.disclosure.values = true;
    value.view.nodes[0].with = { text: "hello", number: 3, bool: true, nil: null, array: [1, "two"], object: { file: "ordinary author field", list: "ordinary author field" } };
    expect(parseRunExport(value).view.nodes[0].with).toEqual(value.view.nodes[0].with);
  });
  test("requires digest-pinned bytes before parsing and refuses redirects or invalid UTF-8", async () => {
    const bytes = new TextEncoder().encode(JSON.stringify(fixture()));
    const fetcher = (async (_url, options) => { expect(options?.redirect).toBe("error"); return new Response(bytes); }) as typeof fetch;
    const pin = { url: "https://assets.example.com/run/manifest.json", sha256: hash(bytes) };
    expect((await readRunExport(pin, undefined, fetcher)).manifest.kind).toBe("fx-run-export-v1");
    await expect(readRunExport({ ...pin, sha256: "c".repeat(64) }, undefined, fetcher)).rejects.toThrow("integrity check");
    const invalid = new Uint8Array([255]);
    await expect(readRunExport({ ...pin, sha256: hash(invalid) }, undefined, (async () => new Response(invalid)) as typeof fetch)).rejects.toThrow("UTF-8 JSON");
    await expect(readRunExport({ ...pin, url: "https://user:password@assets.example.com/manifest.json" }, undefined, fetcher)).rejects.toThrow("unsafe");
    await expect(readRunExport(pin, undefined, (async () => new Response(null, { status: 404 })) as typeof fetch)).rejects.toThrow("HTTP 404");
  });
  test("static image projection is explicit while the local transport remains confined", () => {
    const recording = loaded(withImage());
    const before = JSON.stringify(recording.manifest);
    expect(exportCanvasGraph(recording).nodes[0].preview?.url).toBe(`https://assets.example.com/runs/example/artifacts/${imageDigest}.png`);
    expect(exportArtifactUrl(recording, imageDigest)).toBe(`https://assets.example.com/runs/example/artifacts/${imageDigest}.png`);
    expect(canvasGraph(recording.manifest.view).nodes[0].preview).toBeUndefined();
    expect(JSON.stringify(recording.manifest)).toBe(before);
    const local = structuredClone(recording.manifest.view);
    local.artifacts[0].url = "https://assets.example.com/image.png";
    expect(() => parseViewerRun(local)).toThrow("malformed");
    expect(() => parseRunExport({ ...recording.manifest, view: local })).toThrow();
    expect(exportCanvasGraph(loaded(withImage(true))).nodes[0].preview).toBeUndefined();
  });
  test("a recorded video previews through the same static boundary", () => {
    const value = withImage();
    Object.assign(value.artifacts[0], { media_type: "video/mp4", name: "clip.mp4", path: `artifacts/${imageDigest}.mp4` });
    Object.assign(value.view.artifacts[0], { kind: "video/mp4", name: "clip.mp4" });
    value.view.nodes[0].outputs = { clip: { file: { digest: imageDigest, kind: "video/mp4", name: "clip.mp4", size: 20 } } };
    expect(exportCanvasGraph(loaded(value)).nodes[0].preview).toEqual({
      kind: "video", digest: imageDigest, url: `https://assets.example.com/runs/example/artifacts/${imageDigest}.mp4`, label: "clip: clip.mp4", count: 1,
    });
  });
  test("refuses path escapes, invalid omissions and inventory disagreement", () => {
    for (const path of ["../image.png", "/image.png", "https://example.com/image.png", "artifacts/../image.png", "artifacts/%2e%2e/image.png", "artifacts%2fimage.png", `artifacts/${imageDigest}.png?token=x`, `artifacts/${imageDigest}.png#x`, "artifacts\\image.png", `artifacts/${imageDigest}.webp`]) {
      const value = withImage(); value.artifacts[0].path = path;
      expect(() => parseRunExport(value)).toThrow("unsafe");
    }
    const missing = withImage(); missing.artifacts = [];
    expect(() => parseRunExport(missing)).toThrow("disagree");
    const omission = withImage(true); omission.artifacts[0].omitted_reason = "not_found";
    expect(() => parseRunExport(omission)).toThrow("omission reason");
    const duplicate = withImage(); duplicate.artifacts.push(duplicate.artifacts[0]);
    expect(() => parseRunExport(duplicate)).toThrow("duplicate artifact");
  });
  test("layout preserves inactive declaration cells and unused planned takes without invented ordering", () => {
    const value = fixture();
    value.layout.cells["inactive.child"] = { column: 3, row: 2, source: "automatic" };
    value.view.nodes.push({ ...value.view.nodes[0], id: "prepare#2", take: [2], state: "skipped", cache: null, duration_ms: null });
    expect(parseRunExport(value).view.nodes).toHaveLength(2);
    value.view.nodes[1].duration_ms = 5;
    expect(() => parseRunExport(value)).toThrow("layout order");
  });
  test("rejects silently broken graph and layout references", () => {
    const dependency = fixture(); dependency.view.nodes[0].reads = ["missing#1"];
    expect(() => parseRunExport(dependency)).toThrow("dependency");
    const layout = fixture(); layout.layout.cells = {};
    expect(() => parseRunExport(layout)).toThrow("layout cells");
    const order = fixture(); order.layout.order.push("missing#1");
    expect(() => parseRunExport(order)).toThrow("layout order");
    const value = withImage(); value.view.nodes[0].outputs = { image: { file: { digest: "d".repeat(64), kind: "image/png", name: "missing.png", size: 1 } } };
    expect(() => parseRunExport(value)).toThrow("inventory");
  });
  test("pending repeats preserve recorded node dependencies and permit older absent dependencies", () => {
    const value = fixture();
    value.view.pending = [{ path: "repeat", max: 2, phase: 2, high_usd: 0, waiting_on: ["prepare#1"] }];
    value.layout.cells.repeat = { column: 1, row: 0, source: "automatic" };
    expect(parseRunExport(value).view.pending?.[0].waiting_on).toEqual(["prepare#1"]);
    value.view.pending[0].waiting_on = ["missing#1"];
    expect(() => parseRunExport(value)).toThrow("pending repeat dependency");
    delete value.view.pending[0].waiting_on;
    expect(parseRunExport(value).view.pending).toHaveLength(1);
  });
  test("refuses accidental disclosure and local coordination fields", () => {
    const cursor = fixture(); cursor.layout.cursor = "local-cursor";
    expect(() => parseRunExport(cursor)).toThrow("not portable");
    const values = fixture(); values.view.nodes[0].with = { message: { value: "private" } };
    expect(() => parseRunExport(values)).toThrow("unexpectedly disclosed");
    const diagnostics = fixture(); diagnostics.view.warnings = ["private diagnostic"];
    expect(() => parseRunExport(diagnostics)).toThrow("unexpectedly disclosed");
    const live = fixture(); live.view.nodes[0].state = "running";
    expect(() => parseRunExport(live)).toThrow("still running");
  });
});

describe("static recording controller", () => {
  test("nested imports reuse navigation and restore scope/node deep links", async () => {
    const controller = new ExportViewerController(async () => loaded(nested()));
    await controller.load({ url: "https://assets.example.com/manifest.json", sha256: "a".repeat(64) });
    expect(controller.getSnapshot().graph.nodes.some((node) => node.kind === "workflow")).toBeTrue();
    expect(controller.restore("scope:sub#1", "instance:prepare#1")).toBeTrue();
    expect(controller.getSnapshot().scope).toBe("scope:sub#1");
    expect(controller.getSnapshot().selected).toBe("instance:prepare#1");
    expect(controller.getSnapshot().graph.nodes.find((node) => node.source_id === "prepare#1")?.preview?.url).toContain(imageDigest);
    controller.back();
    expect(controller.getSnapshot().scope).toBeNull();
    expect(controller.restore("missing", null)).toBeFalse();
    controller.dispose();
  });
  test("selection is optional, data loads once, and failed content never leaves a stale graph", async () => {
    let reads = 0;
    const controller = new ExportViewerController(async () => { reads++; return loaded(); });
    await controller.load({ url: "https://assets.example.com/manifest.json", sha256: "a".repeat(64) });
    expect(controller.getSnapshot().selected).toBeNull();
    expect(controller.select("instance:prepare#1")).toBeTrue();
    expect(controller.getSnapshot().selected).toBe("instance:prepare#1");
    expect(controller.select("missing")).toBeFalse();
    expect(controller.select(null)).toBeTrue();
    expect(reads).toBe(1);
    controller.dispose();
    const failed = new ExportViewerController(async () => { throw new Error("Integrity failure"); });
    await failed.load({ url: "https://assets.example.com/manifest.json", sha256: "a".repeat(64) });
    expect(failed.getSnapshot().error).toBe("Integrity failure");
    expect(failed.getSnapshot().graph.nodes).toHaveLength(0);
    expect(failed.getSnapshot().recording).toBeNull();
    failed.dispose();
  });
  test("an older in-flight recording cannot replace a newer selection", async () => {
    let firstResolve!: (value: LoadedRunExport) => void;
    let reads = 0;
    const controller = new ExportViewerController(async () => ++reads === 1 ? new Promise((resolve) => { firstResolve = resolve; }) : loaded());
    const first = controller.load({ url: "https://assets.example.com/first.json", sha256: "a".repeat(64) });
    await controller.load({ url: "https://assets.example.com/second.json", sha256: "b".repeat(64) });
    const stale = loaded(); stale.manifest.view.workflow.title = "Stale";
    firstResolve(stale); await first;
    expect(controller.getSnapshot().recording?.manifest.view.workflow.title).toBe("Synthetic example");
    controller.dispose();
  });
});
