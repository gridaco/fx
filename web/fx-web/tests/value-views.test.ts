import { expect, test } from "bun:test";
import { canvasGraph, layoutGraph, parseGraph, parseViewerRun, ViewerController, ViewerPresentation, valuePreviewLines, valueStateLabel, valueText, valueInspectionData, type ValueView, type ViewerRun } from "../src/index";
import { graph, step } from "./graph.test";
import { nestedPlan } from "./scopes.test";
import { isValueViews, projectValueViews } from "../src/value-views";
const id = (char: string) => `value:${char.repeat(64)}`;
const value = (char: string, changes: Partial<ValueView> = {}): ValueView => ({ id: id(char), scope_id: null, kind: "input", name: "prompt", state: "known", data: { value: "A quiet forest" }, ...changes });

test("legacy none evidence does not claim a genuine null", () => {
  expect(valueText({ none: true })).toBe("No value recorded");
  expect(valueText({ value: null })).toBe("null");
});

test("encoded depth and contextual Unicode match the recording contract", () => {
  let data: unknown = { value: "known" };
  for (let index = 0; index < 31; index++) data = { object: { field: data } };
  expect(isValueViews([value("a", { data, context: "🌿".repeat(256) })])).toBeTrue();
  expect(isValueViews([value("a", { data: { object: { field: data } } })])).toBeFalse();
  expect(isValueViews([value("a", { context: "🌿".repeat(257) })])).toBeFalse();
});

test("plan defaults and connected parameters are effective summaries before a node executes", () => {
  const view = graph();
  view.instances = [step("draw#1", { with: { p1: "hello", p2: "default value", prompt: "A small lantern" }, routes: { "image.generate": { route: "img-a@acme", fingerprint: "a".repeat(64) } } })];
  const node = canvasGraph(parseGraph(view)).nodes[0];
  expect(node.summaries).toContainEqual({ name: "p2", text: "default value" });
  expect(node.summaries?.[0]).toEqual({ name: "prompt", text: "A small lantern" });
  expect(node.route_label).toBe("img-a@acme");
  expect(node.executed).toBeUndefined();
});

test("boundary cards keep their sockets while omitted expressions preserve recorded connections", () => {
  const view = graph();
  view.instances = [step("draw#1", { uses: "draw", with: { prompt: "A quiet forest" } })];
  view.types = { draw: { identity: "fx/draw@1.0", ports: { inputs: {}, outputs: { image: "image/png" }, params: { prompt: { type: "string" } } } } };
  view.value_views = [value("a"), value("b", { kind: "let", name: "brief" }), value("c", { kind: "output", name: "image", state: "pending", data: undefined })];
  delete view.value_views[2].data;
  view.value_links = [
    { source: { kind: "value", value_id: id("a") }, target: { kind: "value", value_id: id("b") } },
    { source: { kind: "value", value_id: id("b") }, target: { kind: "instance", instance_id: "draw#1", port: "prompt" } },
    { source: { kind: "instance", instance_id: "draw#1", port: "image", source_kind: "output" }, target: { kind: "value", value_id: id("c") } },
  ];
  const original = JSON.stringify(view);
  const projected = canvasGraph(parseGraph(view));
  expect(projected.nodes.filter((node) => node.kind === "value").map((node) => node.id)).toEqual([id("a"), id("c")]);
  expect(projected.edges).toHaveLength(2);
  expect(projected.edges).toContainEqual(expect.objectContaining({ source: id("a"), target: "instance:draw#1", target_port: "prompt" }));
  expect(projected.edges.every((edge) => edge.consumed === undefined)).toBeTrue();
  const placed = layoutGraph(projected);
  const at = (key: string) => placed.nodes.find((node) => node.id === key)!;
  expect(at(id("a")).x).toBeLessThan(at("instance:draw#1").x);
  expect(at(id("c")).x).toBeGreaterThan(at("instance:draw#1").x);
  expect(placed.edges.every((edge) => edge.points.length >= 2)).toBeTrue();
  expect(JSON.stringify(view)).toBe(original);
});

test("known null, absence, pending, unavailable and withheld stay distinct", () => {
  expect(valueStateLabel(value("a", { data: { value: null } }))).toBe("null");
  for (const [state, label] of [["absent", "Absent"], ["pending", "Waiting for an upstream result"], ["unavailable", "Value unavailable"], ["withheld", "Withheld by the author"]] as const) {
    expect(valueStateLabel(value("a", { state, data: undefined }))).toBe(label);
  }
  const view = graph();
  view.value_views = [value("a")];
  expect(parseGraph(view).value_views).toEqual(view.value_views);
  expect(() => parseGraph({ ...view, value_views: [{ ...value("a"), state: "pending" }] })).toThrow();
  expect(() => parseGraph({ ...view, value_views: [value("a"), value("a")] })).toThrow();
});

test("file-shaped JSON never becomes a media preview while typed file evidence does", () => {
  const plan = graph();
  const artifact = { digest: "d".repeat(64), name: "reference.png", kind: "image/png", size: 8, available: true, url: `/api/artifacts/${"d".repeat(64)}` };
  const view: ViewerRun = { kind: "fx-viewer-run-v1", workflow: plan.workflow, run_name: "sample", state: "succeeded", stand_in: false, charged_usd: 0, estimate: null, inputs: {}, outputs: {}, nodes: [], warnings: [], artifacts: [artifact],
    value_views: [value("a", { data: { value: { file: artifact } } }), value("b", { name: "reference", data: { file: { digest: artifact.digest, name: artifact.name, kind: artifact.kind, size: artifact.size } } })] };
  const projected = canvasGraph(parseViewerRun(view));
  expect(projected.nodes.find((node) => node.id === id("a"))?.preview).toBeUndefined();
  expect(projected.nodes.find((node) => node.id === id("b"))?.preview).toMatchObject({ kind: "image", url: artifact.url });
  expect(valueText(view.value_views![0].data)).toContain('"file"');
});

test("intermediate contexts stay recorded and only an explicitly inspected occurrence appears", () => {
  const view = nestedPlan();
  view.value_views = [value("a"), value("b", { scope_id: "scope:batch.item#1", kind: "let", name: "brief", context: "warm" }), value("c", { scope_id: "scope:batch.item#1", kind: "let", name: "brief", context: "cool", data: { value: "A cool forest" } })];
  expect(canvasGraph(view).nodes.filter((node) => node.kind === "value").map((node) => node.id)).toEqual([id("a")]);
  const inside = canvasGraph(view, "scope:batch.item#1");
  expect(inside.nodes.filter((node) => node.kind === "value")).toEqual([]);
  const revealed = canvasGraph(view, "scope:batch.item#1", "/api", id("c"));
  expect(revealed.nodes.filter((node) => node.kind === "value").map((node) => node.id)).toEqual([id("c")]);
  expect(revealed.nodes.find((node) => node.id === id("c"))?.value).toEqual(view.value_views[2]);
  expect(layoutGraph(revealed).decks.some((deck) => deck.members.includes(id("b")) || deck.members.includes(id("c")))).toBeFalse();
  expect(canvasGraph(view, null, "/api", id("c")).nodes.some((node) => node.id === id("c"))).toBeFalse();
  expect(view.value_views.map((value) => value.id)).toEqual([id("a"), id("b"), id("c")]);
});

test("value presentation reveals its recorded scope without disclosing contents", async () => {
  const view = nestedPlan();
  view.value_views = [value("a", { scope_id: "scope:batch.item#1", kind: "let", name: "brief", data: { value: "private prompt" } })];
  const controller = new ViewerController(async () => view);
  await controller.refresh();
  expect(controller.getSnapshot().graph.nodes.some((node) => node.id === id("a"))).toBeFalse();
  const presentation = new ViewerPresentation({ getSnapshot: controller.getSnapshot, restore: (scope, selected) => controller.restore(scope, selected) });
  presentation.attachCanvas(() => {});
  const result = presentation.present({ action: "reveal_value", value_id: id("a") });
  expect(result).toMatchObject({ scope_id: "scope:batch.item#1", selection: { kind: "value", value_id: id("a"), scope_id: "scope:batch.item#1" } });
  expect(controller.getSnapshot().graph.nodes.find((node) => node.id === id("a"))?.value).toEqual(view.value_views[0]);
  await controller.refresh();
  expect(controller.getSnapshot().graph.nodes.some((node) => node.id === id("a"))).toBeTrue();
  expect(presentation.getContext().selection).toEqual(result.selection);
  expect(JSON.stringify(result)).not.toContain("private prompt");
  const before = controller.getSnapshot();
  expect(() => presentation.present({ action: "reveal_value", value_id: id("b") })).toThrow("does not exist");
  expect(controller.getSnapshot()).toBe(before);
  expect(presentation.present({ action: "clear_selection" }).selection).toBeNull();
  expect(controller.getSnapshot().graph.nodes.some((node) => node.id === id("a"))).toBeFalse();
  controller.dispose();
});

test("long excerpts have stable bounded geometry independent of byte availability", () => {
  const excerpt = valuePreviewLines("A long prompt. ".repeat(400));
  expect(excerpt.lines).toHaveLength(5);
  expect(excerpt.more).toBeTrue();
  expect(excerpt.lines.every((line) => [...line].length <= 43)).toBeTrue();
  expect(valueText({ value: '${{ inputs.prompt }} <script>literal</script>' })).toBe('${{ inputs.prompt }} <script>literal</script>');
});


test("typed files inside presentation objects retain their explicit variant and opaque JSON stays opaque", () => {
  const file = { file: { digest: "f".repeat(64), kind: "image/png", name: "layer.png", size: 20 } };
  const view = graph();
  view.value_views = [value("a", { name: "layers", data: { list: [{ object: { caption: { value: "Foreground" }, image: file } }] } }), value("b", { data: { value: { object: { image: file } } } })];
  const projected = canvasGraph(parseGraph(view));
  expect(projected.nodes.find((node) => node.id === id("a"))?.value_media).toBeTrue();
  expect(projected.nodes.find((node) => node.id === id("b"))?.value_media).toBeUndefined();
  expect(valueInspectionData(view.value_views[0].data)).toEqual({ list: [{ caption: { value: "Foreground" }, image: file }] });
  expect(valueInspectionData(view.value_views[1].data)).toEqual(view.value_views[1].data);
});

test("a closed import exposes only its own recorded public value sockets", () => {
  const view = nestedPlan(), scope = "scope:batch.item#1";
  view.value_views = [value("a", { name: "reference" }), value("b", { scope_id: scope, name: "image" }), value("c", { scope_id: scope, kind: "output", name: "hero" }), value("d", { kind: "output", name: "result" }), value("e", { scope_id: "scope:batch.item.child#1.1", name: "image" })];
  view.value_links = [
    { source: { kind: "value", value_id: id("a") }, target: { kind: "value", value_id: id("b") } },
    { source: { kind: "value", value_id: id("c") }, target: { kind: "value", value_id: id("d") } },
    { source: { kind: "value", value_id: id("a") }, target: { kind: "value", value_id: id("e") } },
  ];
  const projected = canvasGraph(parseGraph(view));
  const links = projected.edges.filter((edge) => edge.kinds.includes("value"));
  expect(links).toHaveLength(2);
  expect(links).toContainEqual(expect.objectContaining({ source: id("a"), target: scope, target_port: "image" }));
  expect(links).toContainEqual(expect.objectContaining({ source: scope, source_port: "hero", target: id("d") }));
  expect(projected.nodes.some((node) => node.id === id("b") || node.id === id("c") || node.id === id("e"))).toBeFalse();
});

test("hidden value paths preserve named aliases, deduplicate converging contributions and stop cycles", () => {
  const view = graph();
  view.instances = [step("draw#1", { uses: "draw", with: { prompt: "Rendered brief" } })];
  view.types = { draw: { identity: "fx/draw@1.0", ports: { inputs: {}, outputs: { image: "image/png" }, params: { prompt: { type: "string" } } } } };
  view.value_views = [value("a"), value("b", { kind: "let", name: "brief" }), value("c", { kind: "let", name: "variant" }),
    value("d", { kind: "output", name: "hero" }), value("e", { kind: "output", name: "thumbnail" }), value("f", { kind: "table", name: "sizes" })];
  view.value_links = [
    { source: { kind: "value", value_id: id("a") }, target: { kind: "value", value_id: id("b") } },
    { source: { kind: "value", value_id: id("a") }, target: { kind: "value", value_id: id("c") } },
    { source: { kind: "value", value_id: id("b") }, target: { kind: "value", value_id: id("c") } },
    { source: { kind: "value", value_id: id("c") }, target: { kind: "value", value_id: id("b") } },
    { source: { kind: "value", value_id: id("c") }, target: { kind: "instance", instance_id: "draw#1", port: "prompt" } },
    { source: { kind: "instance", instance_id: "draw#1", port: "image", source_kind: "output" }, target: { kind: "value", value_id: id("d") } },
    { source: { kind: "instance", instance_id: "draw#1", port: "image", source_kind: "output" }, target: { kind: "value", value_id: id("e") } },
  ];
  const before = JSON.stringify(view), projected = canvasGraph(parseGraph(view));
  expect(projected.nodes.filter((node) => node.kind === "value").map((node) => node.id)).toEqual([id("a"), id("d"), id("e")]);
  expect(projected.edges).toHaveLength(3);
  expect(projected.edges.filter((edge) => edge.target === "instance:draw#1")).toEqual([expect.objectContaining({ source: id("a"), source_port: "prompt", target_port: "prompt" })]);
  expect(projected.edges.filter((edge) => edge.source === "instance:draw#1").map((edge) => edge.target_port)).toEqual(["hero", "thumbnail"]);
  const draw = projected.nodes.find((node) => node.id === "instance:draw#1")!;
  expect(draw.summaries).not.toContainEqual({ name: "prompt", text: "Rendered brief" });
  expect(draw.ports?.inputs.find((port) => port.name === "prompt")?.inspection?.text).toBe("Rendered brief");
  const revealed = canvasGraph(view, null, "/api", id("b"));
  expect(revealed.nodes.filter((node) => node.kind === "value").map((node) => node.id)).toEqual([id("a"), id("b"), id("d"), id("e")]);
  expect(revealed.edges.some((edge) => edge.source === id("b") && edge.target === "instance:draw#1")).toBeTrue();
  expect(JSON.stringify(view)).toBe(before);
});

test("collapsing a recorded expression path retains an existing processing wire and its consumption evidence", () => {
  const view = graph();
  view.value_views = [value("a", { kind: "let", name: "brief" })];
  view.value_links = [
    { source: { kind: "instance", instance_id: "source#1", port: "text", source_kind: "output" }, target: { kind: "value", value_id: id("a") } },
    { source: { kind: "value", value_id: id("a") }, target: { kind: "instance", instance_id: "draw#1", port: "prompt" } },
  ];
  const source = { id: "instance:source#1", source_id: "source#1", title: "Source", subtitle: "local", state: "succeeded", pending: false,
    ports: { inputs: [], outputs: [{ name: "text", type: "text", kind: "artifact" as const }], settings: [] } };
  const target = { id: "instance:draw#1", source_id: "draw#1", title: "Draw", subtitle: "local", state: "succeeded", pending: false,
    ports: { inputs: [{ name: "prompt", type: "text", kind: "parameter" as const }], outputs: [], settings: [] } };
  const edge = { id: "recorded-read", source: source.id, target: target.id, kind: "data" as const, kinds: ["reads"], consumed: true as const,
    source_port: "text", target_port: "prompt", source_kind: "output" as const };
  const projected = projectValueViews(view, { nodes: [source, target], edges: [edge] }, null, () => null);
  expect(projected.nodes).toHaveLength(2);
  expect(projected.edges).toHaveLength(1);
  expect(projected.edges[0]).toMatchObject(edge);
});

test("hidden traversal cannot cross an imported workflow's private values", () => {
  const view = nestedPlan(), scope = "scope:batch.item#1";
  view.types!.sink.ports!.params.label = { type: "string" };
  view.instances.find((node) => node.id === "save#1")!.with = { label: "A literal setting" };
  view.value_views = [value("a", { name: "reference" }), value("b", { kind: "let", name: "local" }),
    value("c", { scope_id: scope, kind: "let", name: "private" }), value("d", { kind: "output", name: "result" })];
  view.value_links = [
    { source: { kind: "value", value_id: id("a") }, target: { kind: "value", value_id: id("b") } },
    { source: { kind: "value", value_id: id("b") }, target: { kind: "value", value_id: id("c") } },
    { source: { kind: "value", value_id: id("c") }, target: { kind: "value", value_id: id("d") } },
    { source: { kind: "value", value_id: id("c") }, target: { kind: "instance", instance_id: "save#1", port: "label" } },
  ];
  const projected = canvasGraph(parseGraph(view), null, "/api", id("c"));
  expect(projected.nodes.filter((node) => node.kind === "value").map((node) => node.id)).toEqual([id("a"), id("d")]);
  expect(projected.edges.filter((edge) => edge.kinds.includes("value"))).toEqual([]);
  expect(projected.nodes.find((node) => node.source_id === "save#1")?.ports?.settings.map((setting) => setting.name)).toContain("label");
});

test("bounded many-to-many contraction never removes complete recorded processing wiring", () => {
  const view = graph(), count = 130;
  view.value_views = [value("a"), value("b", { kind: "let", name: "combined" })];
  const sources = Array.from({ length: count }, (_, at) => ({ id: `source${at}#1`, source_id: `source${at}#1`, title: "Source", subtitle: "local", state: "succeeded", pending: false,
    ports: { inputs: [], outputs: [{ name: "text", type: "text", kind: "artifact" as const }], settings: [] } }));
  const targets = Array.from({ length: count }, (_, at) => ({ id: `target${at}#1`, source_id: `target${at}#1`, title: "Target", subtitle: "local", state: "succeeded", pending: false,
    ports: { inputs: [{ name: "prompt", type: "text", kind: "parameter" as const }], outputs: [], settings: [] } }));
  view.value_links = [
    { source: { kind: "value", value_id: id("a") }, target: { kind: "value", value_id: id("b") } },
    ...sources.map((node) => ({ source: { kind: "instance" as const, instance_id: node.id, port: "text", source_kind: "output" as const }, target: { kind: "value" as const, value_id: id("b") } })),
    ...targets.map((node) => ({ source: { kind: "value" as const, value_id: id("b") }, target: { kind: "instance" as const, instance_id: node.id, port: "prompt" } })),
  ];
  const raw = { nodes: [...sources, ...targets], edges: sources.flatMap((source) => targets.map((target) => ({ id: `read:${source.id}:${target.id}`, source: source.id, target: target.id,
    kind: "data" as const, kinds: ["reads"], consumed: true as const, source_port: "text", target_port: "prompt", source_kind: "output" as const }))) };
  // Endpoint IDs on canvas processing cards use the same existing instance prefix.
  raw.nodes = raw.nodes.map((node) => ({ ...node, id: `instance:${node.id}` }));
  raw.edges = raw.edges.map((edge) => ({ ...edge, source: `instance:${edge.source}`, target: `instance:${edge.target}` }));
  const projected = projectValueViews(view, raw, null, () => null);
  expect(projected.edges.filter((edge) => edge.id.startsWith("read:"))).toEqual(raw.edges);
  expect(projected.edges.filter((edge) => edge.kinds.includes("value")).length).toBeLessThanOrEqual(16_384);
  expect(new Set(projected.edges.map((edge) => edge.id)).size).toBe(projected.edges.length);
});
