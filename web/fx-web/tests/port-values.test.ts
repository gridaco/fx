import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { canvasGraph, exportCanvasGraph, parseRunExport, type CanvasGraph, type GraphDocument, type LoadedRunExport, type PortInspection, type RunExport, type ViewerRun } from "../src/index";
import { graph, step } from "./graph.test";
import { nestedPlan } from "./scopes.test";

const digest = "d".repeat(64), id = (char: string) => `value:${char.repeat(64)}`;
const at = (graph: CanvasGraph, node: string, direction: "inputs" | "outputs", name: string): PortInspection =>
  graph.nodes.find((item) => item.source_id === node)!.ports![direction].find((port) => port.name === name)!.inspection!;
function plan(): GraphDocument {
  const view = graph();
  view.instances = [step("draw#1", { uses: "draw", with: {} })];
  view.types = { draw: { identity: "fx/draw@1.0", ports: { inputs: { payload: "json", optional: "text?" }, outputs: { image: "image/png", metadata: "json" }, params: {} } } };
  return view;
}
function run(view = plan()): ViewerRun {
  return { kind: "fx-viewer-run-v1", workflow: view.workflow, run_name: "recorded", state: "succeeded", stand_in: true, charged_usd: 0, estimate: null,
    inputs: {}, outputs: {}, artifacts: [], warnings: [], nodes: view.instances.map((item) => ({ id: item.id, path: item.path, title: item.path, uses: item.uses,
      state: "succeeded", reads: item.reads, with: item.with, outputs: {}, cache: "miss", error: null, duration_ms: 1, ports: view.types![item.uses].ports, bindings: item.bindings })) };
}

test("plan sockets show bound literal values without treating JSON-shaped objects as encoded values", () => {
  const view = plan();
  view.instances[0].with = { payload: { value: { file: "literal", withheld: true } }, optional: null };
  const before = JSON.stringify(view), projected = canvasGraph(view);
  expect(at(projected, "draw#1", "inputs", "payload")).toEqual({ state: "known", text: JSON.stringify(view.instances[0].with.payload, null, 2) });
  expect(at(projected, "draw#1", "inputs", "optional")).toEqual({ state: "known", text: "null" });
  expect(at(projected, "draw#1", "outputs", "image").state).toBe("pending");
  expect(JSON.stringify(view)).toBe(before);
  view.instances[0].with.payload = { pending: ["source#1"] };
  view.instances[0].waiting_on = ["source#1"];
  expect(at(canvasGraph(view), "draw#1", "inputs", "payload").state).toBe("pending");
});

test("executed sockets distinguish null, opaque JSON, withholding, absence and unavailable evidence", () => {
  const view = run();
  view.nodes[0].with = { payload: { value: { file: digest, pending: ["literal"], withheld: true } }, optional: { value: null } };
  view.nodes[0].outputs = { metadata: { withheld: true } };
  let projected = canvasGraph(view);
  expect(at(projected, "draw#1", "inputs", "payload")).toEqual({ state: "known", text: JSON.stringify({ file: digest, pending: ["literal"], withheld: true }, null, 2) });
  expect(at(projected, "draw#1", "inputs", "optional")).toEqual({ state: "known", text: "null" });
  expect(at(projected, "draw#1", "outputs", "metadata").state).toBe("withheld");
  expect(at(projected, "draw#1", "outputs", "image").state).toBe("absent");
  view.nodes[0].with.payload = { missing: true };
  view.nodes[0].with.optional = { none: true };
  projected = canvasGraph(view);
  expect(at(projected, "draw#1", "inputs", "payload").state).toBe("absent");
  expect(at(projected, "draw#1", "inputs", "optional").state).toBe("unavailable");
  Object.assign(view.nodes[0], { state: "pending", duration_ms: null, cache: null, outputs: {}, with: { payload: { withheld: true } } });
  expect(at(canvasGraph(view), "draw#1", "inputs", "payload").state).toBe("withheld");
  expect(at(canvasGraph(view), "draw#1", "outputs", "image").state).toBe("pending");
});

test("typed files use recorded metadata while unwrapped authored file-looking JSON stays literal", () => {
  const view = run();
  view.artifacts = [{ digest, kind: "image/png", name: "reference.png", size: 123, available: false, url: null }];
  view.nodes[0].with = { payload: { file: digest } };
  const outputDigest = "e".repeat(64);
  view.nodes[0].outputs = { image: { file: { digest: outputDigest, name: "rendered.png", kind: "image/png", size: 456 } } };
  const projected = canvasGraph(view);
  expect(at(projected, "draw#1", "inputs", "payload")).toEqual({ state: "known", text: "reference.png\nimage/png · 123 bytes",
    images: [{ digest, name: "reference.png", kind: "image/png", size: 123, url: null }], image_count: 1 });
  expect(at(projected, "draw#1", "outputs", "image")).toEqual({ state: "known", text: "rendered.png\nimage/png · 456 bytes",
    images: [{ digest: outputDigest, name: "rendered.png", kind: "image/png", size: 456, url: null }], image_count: 1 });
  view.nodes[0].with.payload = { file: "e".repeat(64) };
  expect(at(canvasGraph(view), "draw#1", "inputs", "payload").state).toBe("unavailable");
});

test("image sockets resolve only their own typed images and scoped inventory URLs", () => {
  const view = run(), api = `/p/${"a".repeat(64)}/runs/${"b".repeat(64)}/api`;
  const other = "e".repeat(64), local = `${api}/artifacts/${digest}`;
  view.artifacts = [{ digest, kind: "image/png", name: "input.png", size: 12, available: true, url: local },
    { digest: other, kind: "image/png", name: "output.png", size: 24, available: true, url: `${api}/artifacts/${other}` }];
  view.nodes[0].with = { payload: { file: digest }, optional: { value: { file: digest } } };
  view.nodes[0].outputs = { image: { file: other }, metadata: { value: { file: { digest: other, kind: "image/png", url: "https://invalid.example/private.png" } } } };
  let projected = canvasGraph(view, null, api);
  expect(at(projected, "draw#1", "inputs", "payload").images?.map((image) => [image.name, image.url])).toEqual([["input.png", local]]);
  expect(at(projected, "draw#1", "outputs", "image").images?.map((image) => image.digest)).toEqual([other]);
  expect(at(projected, "draw#1", "inputs", "optional").images).toBeUndefined();
  expect(at(projected, "draw#1", "outputs", "metadata").images).toBeUndefined();
  view.artifacts[0].url = `/api/artifacts/${digest}`;
  expect(at(canvasGraph(view, null, api), "draw#1", "inputs", "payload").images?.[0].url).toBeNull();
  view.artifacts[0].url = "https://invalid.example/private.png";
  projected = canvasGraph(view, null, api);
  expect(at(projected, "draw#1", "inputs", "payload").images?.[0].url).toBeNull();
  view.nodes[0].with.payload = { withheld: true };
  expect(at(canvasGraph(view, null, api), "draw#1", "inputs", "payload").images).toBeUndefined();
});

test("typed image collections preserve key order and bound thumbnails without inspecting opaque objects", () => {
  const view = run();
  const files = Array.from({ length: 14 }, (_, at) => ({ file: { digest: at.toString(16).padStart(64, "0"), name: `image-${at}.png`, kind: "image/png", size: at + 1 } }));
  view.nodes[0].outputs.image = { collection: files.map((file, at) => [`shot-${at}`, file]) };
  const gallery = at(canvasGraph(view), "draw#1", "outputs", "image");
  expect(gallery.image_count).toBe(14);
  expect(gallery.images).toHaveLength(12);
  expect(gallery.images?.map((image) => image.key)).toEqual(files.slice(0, 12).map((_, at) => `shot-${at}`));
  expect(gallery.images?.every((image) => image.url === null)).toBeTrue();
  view.nodes[0].outputs.image = { object: { hero: files[0], detail: { list: [files[1], { value: { file: files[2].file } }] } } };
  const object = at(canvasGraph(view), "draw#1", "outputs", "image");
  expect(object.images?.map((image) => [image.name, image.key])).toEqual([["image-0.png", "hero"], ["image-1.png", "detail/0"]]);
  view.nodes[0].outputs.image = { file: { digest, name: "unsupported.svg", kind: "image/svg+xml", size: 20 } };
  expect(at(canvasGraph(view), "draw#1", "outputs", "image").images).toBeUndefined();
});

test("plan raw file-looking data and closed imported internals cannot create image galleries", () => {
  const source = plan();
  source.instances[0].with.payload = { file: { digest, name: "literal.png", kind: "image/png", size: 1 } };
  expect(at(canvasGraph(source), "draw#1", "inputs", "payload").images).toBeUndefined();
  const nested = nestedPlan(), view = run(nested), scope = "scope:batch.item#1";
  view.scopes = nested.scopes;
  view.artifacts = [{ digest, name: "private.png", kind: "image/png", size: 1, available: true, url: `/api/artifacts/${digest}` }];
  view.nodes.find((node) => node.id === "a.paint#1")!.outputs.image = { file: digest };
  expect(at(canvasGraph(view), scope, "outputs", "hero").images).toBeUndefined();
  view.value_views = [{ id: id("a"), scope_id: scope, kind: "output", name: "hero", state: "known", data: { file: { digest, name: "private.png", kind: "image/png", size: 1 } } }];
  expect(at(canvasGraph(view), scope, "outputs", "hero").images?.[0].url).toBe(`/api/artifacts/${digest}`);
  expect(at(canvasGraph(view), scope, "outputs", "alternate").images).toBeUndefined();
});

test("exported image sockets resolve bundle URLs and preserve unavailable metadata", () => {
  const manifest: RunExport = JSON.parse(readFileSync(new URL("../../../spec/vectors/exports/minimal.json", import.meta.url), "utf8"));
  const file = { digest, name: "preview.png", kind: "image/png", size: 20 };
  manifest.artifacts = [{ digest, name: file.name, media_type: file.kind, size: file.size, path: `artifacts/${digest}.png`, omitted_reason: null }];
  manifest.view.artifacts = [{ ...file, available: true, url: null }];
  manifest.view.nodes[0].ports = { inputs: { reference: "image/png" }, outputs: { image: "image/png" }, params: {} };
  manifest.view.nodes[0].with = { reference: { file } };
  manifest.view.nodes[0].outputs = { image: { file } };
  const loaded: LoadedRunExport = { manifest: parseRunExport(manifest), manifest_url: "https://assets.example.com/runs/example/manifest.json" };
  const shared = exportCanvasGraph(loaded), local = canvasGraph(loaded.manifest.view);
  expect(at(shared, "prepare#1", "inputs", "reference").images?.[0].url).toBe(`https://assets.example.com/runs/example/artifacts/${digest}.png`);
  expect(at(shared, "prepare#1", "outputs", "image").images?.[0].url).toBe(`https://assets.example.com/runs/example/artifacts/${digest}.png`);
  expect(at(local, "prepare#1", "outputs", "image").images).toEqual(at(shared, "prepare#1", "outputs", "image").images?.map((image) => ({ ...image, url: null })));
  manifest.artifacts[0].path = null;
  manifest.artifacts[0].omitted_reason = "unplaced_input";
  manifest.view.artifacts[0].available = false;
  const unavailable = exportCanvasGraph({ ...loaded, manifest: parseRunExport(manifest) });
  expect(at(unavailable, "prepare#1", "outputs", "image").images).toEqual([{ ...file, url: null }]);
});

test("a fact socket never borrows a same-named output's recorded value", () => {
  const source = plan();
  source.types!.draw.ports!.outputs.score = "json";
  source.instances.push(step("read#1", { uses: "draw", with: { payload: 99 }, bindings: [{ source: "draw#1", source_port: "score", target_port: "payload", source_kind: "fact" }] }));
  const view = run(source);
  view.nodes[0].outputs = { score: { value: 99 } };
  const ports = canvasGraph(view).nodes[0].ports!.outputs.filter((port) => port.name === "score");
  expect(ports.find((port) => port.kind === "artifact")!.inspection).toEqual({ state: "known", text: "99" });
  expect(ports.find((port) => port.kind === "fact")!.inspection!.state).toBe("unavailable");
});

test("connected fields move to sockets and remaining settings are recomputed from the full binding", () => {
  const view = plan();
  view.types!.draw.ports = { inputs: {}, outputs: { image: "image/png" }, params: Object.fromEntries(["prompt", "a", "b", "c", "quality", "seed"].map((name) => [name, { type: "string" }])) };
  view.types!.draw.ports.params.prompt.description = "Describe the image to generate.";
  view.instances[0].with = { prompt: "A long prompt", a: "One", b: "Two", c: "Three", quality: "max", seed: 42 };
  view.value_views = [{ id: id("a"), scope_id: null, kind: "input", name: "brief", state: "known", data: { value: "Source text" } }];
  view.value_links = ["prompt", "a", "b", "c"].map((port) => ({ source: { kind: "value", value_id: id("a") }, target: { kind: "instance", instance_id: "draw#1", port } }));
  const node = canvasGraph(view).nodes.find((node) => node.source_id === "draw#1")!;
  expect(node.summaries).toEqual([{ name: "quality", text: "max" }, { name: "seed", text: "42" }]);
  expect(node.summary_count).toBe(2);
  expect(node.ports!.inputs.map((port) => port.name)).toEqual(["prompt", "a", "b", "c"]);
  expect(node.ports!.inputs[0].inspection).toEqual({ state: "known", text: "A long prompt", description: "Describe the image to generate.", sources: ["Input brief"] });
  const recorded = run(view);
  recorded.value_views = view.value_views;
  recorded.value_links = view.value_links;
  expect(at(canvasGraph(recorded), "draw#1", "inputs", "prompt").description).toBe("Describe the image to generate.");
  recorded.nodes[0].ports!.params.prompt.description = "   ";
  expect(at(canvasGraph(recorded), "draw#1", "inputs", "prompt").description).toBeUndefined();
});

test("parameter descriptions never substitute source metadata or appear on file/output sockets", () => {
  const view = plan();
  view.types!.draw.ports!.params.prompt = { type: "string", description: "The exact parameter description." };
  view.types!.draw.ports!.outputs.prompt = "text";
  view.instances[0].with = { payload: "Reference", prompt: "Effective prompt" };
  view.value_views = [{ id: id("a"), scope_id: null, kind: "let", name: "brief", state: "known", data: { value: { description: "Source metadata must not be borrowed." } } },
    { id: id("b"), scope_id: null, kind: "input", name: "source", state: "known", data: { value: "Source text" } }];
  view.value_links = [{ source: { kind: "value", value_id: id("b") }, target: { kind: "value", value_id: id("a") } },
    ...["payload", "prompt"].map((port) => ({ source: { kind: "value" as const, value_id: id("a") }, target: { kind: "instance" as const, instance_id: "draw#1", port } }))];
  const projected = canvasGraph(view);
  expect(at(projected, "draw#1", "inputs", "prompt").description).toBe("The exact parameter description.");
  expect(at(projected, "draw#1", "inputs", "payload").description).toBeUndefined();
  expect(at(projected, "draw#1", "outputs", "prompt").description).toBeUndefined();
  expect(at(projected, "draw#1", "outputs", "image").description).toBeUndefined();
  delete view.types!.draw.ports!.params.prompt.description;
  expect(at(canvasGraph(view), "draw#1", "inputs", "prompt").description).toBeUndefined();
});

test("a hidden constant retains its consumer socket without a source card or invented wire", () => {
  const view = plan();
  view.types!.draw.ports!.params = { style: { type: "string", description: "The style to render." }, quality: { type: "string" } };
  view.instances[0].with = { style: "sticker", quality: "max" };
  view.value_views = [{ id: id("a"), scope_id: null, kind: "table", name: "styles", state: "known", data: { value: { sticker: "Flat colors" } } }];
  view.value_links = [{ source: { kind: "value", value_id: id("a") }, target: { kind: "instance", instance_id: "draw#1", port: "style" } }];
  const projected = canvasGraph(view), node = projected.nodes[0];
  expect(projected.nodes.filter((node) => node.kind === "value")).toEqual([]);
  expect(projected.edges).toEqual([]);
  expect(at(projected, "draw#1", "inputs", "style")).toEqual({ state: "known", text: "sticker", description: "The style to render.", sources: ["Constant styles"] });
  expect(node.ports!.settings.map((setting) => setting.name)).toEqual(["quality"]);
  expect(node.summaries).toEqual([{ name: "quality", text: "max" }]);
});

test("provenance hints retain distinct contexts but a closed import exposes only its owned public values", () => {
  const view = nestedPlan(), scope = "scope:batch.item#1";
  view.instances.find((node) => node.id === "save#1")!.with = { image: "effective value" };
  view.value_views = [
    { id: id("a"), scope_id: scope, kind: "input", name: "image", state: "known", data: { value: "Public reference" } },
    { id: id("b"), scope_id: "scope:a.inline#1", kind: "let", name: "secret", state: "known", data: { value: "Private prompt" } },
    { id: id("c"), scope_id: null, kind: "let", name: "brief", context: "warm", state: "known", data: { value: "Warm" } },
    { id: id("d"), scope_id: null, kind: "let", name: "brief", context: "cool", state: "known", data: { value: "Cool" } },
  ];
  view.value_links = ["b", "c", "d"].map((char) => ({ source: { kind: "value", value_id: id(char) }, target: { kind: "instance", instance_id: "save#1", port: "image" } }));
  const projected = canvasGraph(view);
  expect(at(projected, scope, "inputs", "image")).toEqual({ state: "known", text: "Public reference" });
  expect(at(projected, "save#1", "inputs", "image")).toEqual({ state: "known", text: "effective value", sources: ["Expression brief · warm", "Expression brief · cool"] });
  expect(JSON.stringify(projected.nodes.find((node) => node.source_id === scope)?.ports)).not.toContain("Private prompt");
  expect(JSON.stringify(at(projected, "save#1", "inputs", "image"))).not.toContain("secret");
});

test("static exports and local projections expose the same disclosed socket values", () => {
  const manifest: RunExport = JSON.parse(readFileSync(new URL("../../../spec/vectors/exports/minimal.json", import.meta.url), "utf8"));
  manifest.disclosure.values = true;
  const node = manifest.view.nodes[0];
  node.ports = { inputs: {}, outputs: { metadata: "json" }, params: { message: { type: "string", description: "A message to prepare." } } };
  node.with = { message: { value: "A complete multiline\nmessage" } };
  node.outputs = { metadata: { value: { done: true } } };
  manifest.view.value_views = [{ id: id("a"), scope_id: null, kind: "input", name: "message", state: "known", data: { value: "A complete multiline\nmessage" } }];
  manifest.view.value_links = [{ source: { kind: "value", value_id: id("a") }, target: { kind: "instance", instance_id: node.id, port: "message" } }];
  const loaded: LoadedRunExport = { manifest: parseRunExport(manifest), manifest_url: "https://assets.example.com/runs/example/manifest.json" };
  const local = canvasGraph(loaded.manifest.view), shared = exportCanvasGraph(loaded);
  expect(shared.nodes[0].ports).toEqual(local.nodes[0].ports);
  expect(at(shared, node.id, "inputs", "message").text).toBe("A complete multiline\nmessage");
  expect(at(shared, node.id, "inputs", "message").description).toBe("A message to prepare.");
  expect(at(shared, node.id, "outputs", "metadata")).toEqual({ state: "known", text: '{\n  "done": true\n}' });
  // An undisclosed export supplies only structural schema, never current-source annotations.
  manifest.disclosure.values = false;
  node.ports.params.message = { type: "string" };
  node.with.message = { withheld: true };
  node.outputs.metadata = { withheld: true };
  manifest.view.value_views = [{ id: id("a"), scope_id: null, kind: "input", name: "message", state: "withheld" }];
  const withheld = exportCanvasGraph({ ...loaded, manifest: parseRunExport(manifest) });
  expect(at(withheld, node.id, "inputs", "message").description).toBeUndefined();
  expect(at(withheld, node.id, "inputs", "message").state).toBe("withheld");
});

test("socket text is bounded without changing known data or fetching file contents", () => {
  const view = plan();
  view.instances[0].with.payload = "🌿".repeat(40_000);
  const before = JSON.stringify(view), inspection = at(canvasGraph(view), "draw#1", "inputs", "payload");
  expect(inspection.state).toBe("known");
  expect(inspection.text.length).toBeLessThanOrEqual(64 * 1024 + 2);
  expect(inspection.text.endsWith("\n…")).toBeTrue();
  expect(inspection.text.at(-3)).not.toMatch(/[\uD800-\uDBFF]/);
  expect(JSON.stringify(view)).toBe(before);
});
