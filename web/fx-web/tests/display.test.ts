import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  canvasGraph, DEFAULT_VIEWER_DISPLAY, ExportViewerController, exportCanvasGraph, layoutGraph,
  parseRunExport, parseViewerConfig, readViewerConfig, ViewerController, viewerDisplay,
  type GraphDocument, type LoadedRunExport, type RunExport, type ViewerDisplay, type WorkflowNote,
} from "../src/index";
import { graph, step } from "./graph.test";
import { nestedPlan } from "./scopes.test";

const inputId = `value:${"a".repeat(64)}`, outputId = `value:${"b".repeat(64)}`;
const hidden = viewerDisplay({ workflow_inputs: false, workflow_outputs: false, notes: false });
const note = (scope: string | null = null): WorkflowNote => ({ id: `note:${scope ?? "root"}:guide`, name: "guide", scope_id: scope, about: null, format: "text", text: "Read the recorded reference." });
function plan(): GraphDocument {
  const value = graph();
  value.instances = [step("draw#1", { uses: "draw", with: { prompt: "A quiet forest", size: 256 } })];
  value.pending = [];
  value.types = { draw: { identity: "fx/draw@1.0", ports: { inputs: {}, outputs: { image: "image/png" }, params: { prompt: { type: "string" }, size: { type: "integer" } } } } };
  value.notes = [note()];
  value.value_views = [
    { id: inputId, scope_id: null, kind: "input", name: "prompt", state: "known", data: { value: "A quiet forest" } },
    { id: outputId, scope_id: null, kind: "output", name: "image", state: "pending" },
  ];
  value.value_links = [
    { source: { kind: "value", value_id: inputId }, target: { kind: "instance", instance_id: "draw#1", port: "prompt" } },
    { source: { kind: "instance", instance_id: "draw#1", port: "image", source_kind: "output" }, target: { kind: "value", value_id: outputId } },
  ];
  return value;
}
function recording(): LoadedRunExport {
  const manifest: RunExport = JSON.parse(readFileSync(new URL("../../../spec/vectors/exports/minimal.json", import.meta.url), "utf8"));
  manifest.disclosure.values = true;
  manifest.view.value_views = plan().value_views;
  manifest.view.notes = [note()];
  manifest.layout.notes = { [note().id]: { column: 0, row: 1, source: "automatic" } };
  return { manifest: parseRunExport(manifest), manifest_url: "https://assets.example.test/run/manifest.json" };
}

describe("viewer display configuration", () => {
  test("defaults are complete and strict configuration never treats strings or unknown options as booleans", () => {
    expect(viewerDisplay()).toEqual({ workflow_inputs: true, workflow_outputs: true, notes: true });
    expect(viewerDisplay({ workflow_outputs: false })).toEqual({ ...DEFAULT_VIEWER_DISPLAY, workflow_outputs: false });
    expect(parseViewerConfig({ display: hidden })).toEqual({ display: hidden });
    for (const malformed of [null, {}, { display: {} }, { display: { ...hidden, notes: "false" } }, { display: { ...hidden, step_media_previews: false } }, { display: hidden, extra: true }]) {
      expect(() => parseViewerConfig(malformed)).toThrow("configuration");
    }
    for (const malformed of [null, { notes: "false" }, { step_media_previews: false }]) expect(() => viewerDisplay(malformed as Partial<ViewerDisplay>)).toThrow("display options");
  });

  test("reads only its scoped same-origin endpoint without caching; only a missing older endpoint uses defaults", async () => {
    const api = `/p/${"c".repeat(64)}/runs/${"d".repeat(64)}/api`, signal = new AbortController().signal;
    const fetcher = (async (url, options) => {
      expect(url).toBe(`${api}/viewer_config`);
      expect(options).toMatchObject({ signal, cache: "no-store" });
      return Response.json({ display: hidden });
    }) as typeof fetch;
    expect(await readViewerConfig(signal, api, fetcher)).toEqual({ display: hidden });
    expect(await readViewerConfig(undefined, "/api", (async () => new Response(null, { status: 404 })) as typeof fetch)).toEqual({ display: viewerDisplay() });
    await expect(readViewerConfig(undefined, "/api", (async () => new Response(null, { status: 422 })) as typeof fetch)).rejects.toThrow("configuration");
    await expect(readViewerConfig(undefined, "/api", (async () => Response.json({ display: { notes: false } })) as typeof fetch)).rejects.toThrow("malformed");
    await expect(readViewerConfig(undefined, "/api", (async () => new Response("<html>Viewer</html>")) as typeof fetch)).rejects.toThrow("configuration");
    await expect(readViewerConfig(undefined, "https://other.example.test/api", fetcher)).rejects.toThrow("supported");
  });
});

test("input/output visibility is independent and does not change step media, values or socket inspection", () => {
  const view = plan(), original = JSON.stringify(view), all = canvasGraph(view);
  const withoutInputs = canvasGraph(view, null, "/api", null, viewerDisplay({ workflow_inputs: false }));
  expect(withoutInputs.nodes.map((node) => node.id)).toEqual(["instance:draw#1", outputId]);
  expect(withoutInputs.edges.every((edge) => edge.source !== inputId && edge.target !== inputId)).toBeTrue();
  const draw = withoutInputs.nodes[0], before = all.nodes.find((node) => node.id === draw.id)!;
  expect(draw).toEqual(before);
  expect(draw.summaries).toContainEqual({ name: "size", text: "256" });
  expect(draw.ports?.inputs.find((port) => port.name === "prompt")?.inspection?.text).toBe("A quiet forest");
  const withoutOutputs = canvasGraph(view, null, "/api", null, viewerDisplay({ workflow_outputs: false }));
  expect(withoutOutputs.nodes.map((node) => node.id)).toEqual(["instance:draw#1", inputId]);
  expect(withoutOutputs.notes).toEqual(all.notes);
  expect(JSON.stringify(view)).toBe(original);
});

test("hidden boundary groups and notes contribute no layout bounds, including supplied note coordinates", () => {
  const view = plan(), shown = canvasGraph(view), projected = canvasGraph(view, null, "/api", null, hidden);
  const layout = layoutGraph(projected), ordinary = layoutGraph(shown);
  expect(projected.nodes.map((node) => node.id)).toEqual(["instance:draw#1"]);
  expect(projected.notes).toEqual([]);
  expect(layout.boundary_groups ?? []).toEqual([]);
  expect(layout.notes).toEqual([]);
  expect(layout.width).toBeLessThan(ordinary.width);
  const onlyStep = { ...shown, nodes: shown.nodes.filter((node) => node.kind !== "value"), edges: [], notes: [] };
  expect(layout).toEqual(layoutGraph(onlyStep));
});

test("group notes and imported workflow boundaries use the same display policy, including legacy boundaries", () => {
  const view = nestedPlan(), scope = "scope:batch.item#1";
  view.notes = [note(), note("scope:batch#1"), note(scope)];
  expect(canvasGraph(view, null, "/api", null, hidden).notes).toEqual([]);
  const legacy = canvasGraph(view, scope);
  expect(legacy.nodes.filter((node) => node.kind === "boundary").map((node) => node.side)).toEqual(["input", "output"]);
  expect(canvasGraph(view, scope, "/api", null, viewerDisplay({ workflow_inputs: false })).nodes.filter((node) => node.kind === "boundary").map((node) => node.side)).toEqual(["output"]);
  expect(canvasGraph(view, scope, "/api", null, hidden).nodes.every((node) => node.kind !== "boundary")).toBeTrue();
  expect(canvasGraph(view, scope, "/api", null, hidden).notes).toEqual([]);
});

test("custom-reader controllers preserve explicit hidden-item inspection and remove it when selection changes", async () => {
  const controller = new ViewerController(async () => plan(), { display: hidden });
  await controller.refresh();
  expect(controller.getSnapshot().graph.notes).toEqual([]);
  expect(controller.restore(null, note().id)).toBeTrue();
  expect(controller.getSnapshot().graph.notes?.map((item) => item.id)).toEqual([note().id]);
  controller.select("instance:draw#1");
  expect(controller.getSnapshot().graph.notes).toEqual([]);
  expect(controller.restore(null, inputId)).toBeTrue();
  expect(controller.getSnapshot().graph.nodes.some((node) => node.id === inputId)).toBeTrue();
  controller.select(null);
  expect(controller.getSnapshot().graph.nodes.some((node) => node.id === inputId)).toBeFalse();
  controller.dispose();
});

test("native controllers wait for resolved configuration before first projection and refresh it without executing source", async () => {
  const original = globalThis.fetch;
  let resolve!: (response: Response) => void, config = viewerDisplay({ workflow_inputs: false });
  let delayed = true;
  globalThis.fetch = (async (url) => {
    if (String(url) === "/api/view") return Response.json(plan());
    expect(String(url)).toBe("/api/viewer_config");
    return delayed ? new Promise<Response>((done) => { resolve = done; }) : Response.json({ display: config });
  }) as typeof fetch;
  const controller = new ViewerController(undefined, { observation: false, layout: false });
  try {
    const loading = controller.refresh();
    await Bun.sleep(0);
    expect(controller.getSnapshot().view).toBeNull();
    resolve(Response.json({ display: config }));
    await loading;
    expect(controller.getSnapshot().error).toBeNull();
    expect(controller.getSnapshot().graph.nodes.some((node) => node.id === inputId)).toBeFalse();
    delayed = false; config = viewerDisplay();
    await controller.refresh();
    expect(controller.getSnapshot().graph.nodes.some((node) => node.id === inputId)).toBeTrue();
  } finally { controller.dispose(); globalThis.fetch = original; }
});

test("invalid configuration is surfaced instead of projecting new evidence with guessed visibility", async () => {
  const controller = new ViewerController(async () => plan(), { viewerConfig: async () => ({ display: { notes: false } }) as never });
  await controller.refresh();
  expect(controller.getSnapshot()).toMatchObject({ view: null, loading: false, error: expect.stringContaining("configuration") });
  controller.dispose();
});

test("a stale configuration response cannot override a newer refresh", async () => {
  const pending: ((value: { display: ViewerDisplay }) => void)[] = [];
  const controller = new ViewerController(async () => plan(), { viewerConfig: () => new Promise((done) => pending.push(done)) });
  const first = controller.refresh(), second = controller.refresh();
  pending[1]({ display: hidden }); await second;
  pending[0]({ display: viewerDisplay() }); await first;
  expect(controller.getSnapshot().graph.nodes.map((node) => node.id)).toEqual(["instance:draw#1"]);
  controller.dispose();
});

test("portable viewers use frozen display defaults and retain recorded content for explicit inspection", async () => {
  const loaded = recording(), before = JSON.stringify(loaded.manifest);
  expect(exportCanvasGraph(loaded).nodes.filter((node) => node.kind === "value")).toHaveLength(2);
  loaded.manifest.viewer = { display: hidden };
  const configured = JSON.stringify(loaded.manifest);
  expect(exportCanvasGraph(loaded).nodes.filter((node) => node.kind === "value")).toHaveLength(0);
  expect(exportCanvasGraph(loaded).notes).toEqual([]);
  expect(exportCanvasGraph(loaded, null, null, { workflow_outputs: true }).nodes.some((node) => node.id === outputId)).toBeTrue();
  const controller = new ExportViewerController(async () => loaded);
  await controller.load({ url: loaded.manifest_url, sha256: "c".repeat(64) });
  expect(controller.restore(null, note().id)).toBeTrue();
  controller.select(null);
  expect(controller.getSnapshot().graph.notes).toEqual([]);
  expect(controller.restore(null, outputId)).toBeTrue();
  controller.select(null);
  expect(controller.getSnapshot().graph.nodes.some((node) => node.id === outputId)).toBeFalse();
  expect(JSON.stringify(loaded.manifest)).toBe(configured);
  expect(before).not.toBe(configured);
  controller.dispose();
  expect(() => parseRunExport({ ...loaded.manifest, viewer: { display: { ...hidden, notes: "false" } } })).toThrow("configuration");
});
