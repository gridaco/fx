import { expect, test } from "bun:test";
import minimalExport from "../../../../spec/vectors/exports/minimal.json";
import { exportCanvasGraph, parseRunExport, type LoadedRunExport, type RunExport, type ValueView } from "@grida/fx-web";
import { canvasFragment, isWithheldValue, parseCanvasFragment, RecordedCanvasController } from "./recorded-canvas-controller";

test("Overview / Canvas fragments retain exact recording, scope and node identities", () => {
  const fragment = canvasFragment("world-01", "scope:world.branch#1", "instance:paint[blue]#2");
  expect(parseCanvasFragment(fragment)).toEqual({ graph: true, recording: "world-01", scope: "scope:world.branch#1", node: "instance:paint[blue]#2" });
  expect(parseCanvasFragment("#graph")).toEqual({ graph: true, recording: null, scope: null, node: null });
  expect(parseCanvasFragment("#usage-title").graph).toBeFalse();
  expect(parseCanvasFragment("").graph).toBeFalse();
});

test("an included author's ordinary JSON object is not mistaken for a disclosure marker", () => {
  expect(isWithheldValue({ withheld: true }, false)).toBeTrue();
  expect(isWithheldValue({ withheld: true }, true)).toBeFalse();
  expect(isWithheldValue({ withheld: false }, false)).toBeFalse();
  expect(isWithheldValue(null, false)).toBeFalse();
});

/** Minimal document surface for the real inspector renderer, without canvas/browser setup. */
class InspectorElement {
  children: InspectorElement[] = [];
  textContent = "";
  hidden = false;
  constructor(readonly tagName = "div") {}
  append(...nodes: InspectorElement[]) { this.children.push(...nodes); }
  replaceChildren(...nodes: InspectorElement[]) { this.children = nodes; }
  addEventListener() {}
  text(): string { return [this.textContent, ...this.children.map((node) => node.text())].filter(Boolean).join("\n"); }
  elements(tag: string): InspectorElement[] { return [...(this.tagName === tag ? [this] : []), ...this.children.flatMap((node) => node.elements(tag))]; }
}

function inspectValue(value: Partial<ValueView>, configure?: (manifest: RunExport) => void) {
  const manifest = structuredClone(minimalExport) as RunExport;
  const selected: ValueView = { id: `value:${"a".repeat(64)}`, scope_id: null, kind: "input", name: "prompt", state: "known", ...value };
  manifest.disclosure.values = selected.state !== "withheld";
  manifest.view.value_views = [selected];
  manifest.layout.values = { [selected.id]: { column: 0, row: 0, source: "automatic" } };
  configure?.(manifest);
  const loaded: LoadedRunExport = { manifest: parseRunExport(manifest), manifest_url: "https://assets.example.test/recording/manifest.json" };
  const inspector = new InspectorElement(), body = new InspectorElement();
  const root = {
    ownerDocument: { createElement: (tag: string) => new InspectorElement(tag) },
    querySelector: (selector: string) => selector === "[data-inspector]" ? inspector : body,
  };
  const controller = Object.create(RecordedCanvasController.prototype);
  Reflect.set(controller, "root", root);
  Reflect.set(controller, "abort", new AbortController());
  Reflect.apply(Reflect.get(controller, "renderInspector"), controller, [{
    graph: exportCanvasGraph(loaded), selected: selected.id, recording: loaded,
  }]);
  expect(inspector.hidden).toBeFalse();
  return body;
}

test("published value inspection shows the entire prompt and treats opaque JSON as text", () => {
  const prompt = "A long reference description. ".repeat(100) + "<script>literal text</script>";
  const body = inspectValue({ data: { value: prompt }, context: "warm" });
  expect(body.text()).toContain(prompt);
  expect(body.text()).toContain("Input · warm");
  expect(body.elements("script")).toEqual([]);
  expect(body.elements("pre")[0].textContent).toBe(prompt);
  const opaque = { file: { digest: "f".repeat(64), kind: "image/png" }, list: [1, 2], withheld: true };
  const json = inspectValue({ data: { value: opaque } });
  expect(json.elements("pre")[0].textContent).toBe(JSON.stringify(opaque, null, 2));
  expect(json.elements("img")).toEqual([]);
  expect(json.elements("a")).toEqual([]);
});

test("published value inspection preserves nested files and labels omitted input bytes", () => {
  const file = { digest: "b".repeat(64), name: "reference.png", kind: "image/png", size: 20 };
  const missing = { ...file, digest: "c".repeat(64), name: "original.png" };
  const body = inspectValue({ kind: "output", data: { object: {
    caption: { value: "Reference <caption>" },
    layers: { list: [{ file }, { collection: [["original", { file: missing }]] }] },
  } } }, (manifest) => {
    manifest.artifacts = [
      { digest: file.digest, media_type: file.kind, name: file.name, size: file.size, path: `artifacts/${file.digest}.png`, omitted_reason: null },
      { digest: missing.digest, media_type: missing.kind, name: missing.name, size: missing.size, path: null, omitted_reason: "unplaced_input" },
    ];
    manifest.view.artifacts = [{ ...file, available: true, url: null }, { ...missing, available: false, url: null }];
  });
  expect(body.text()).toContain("Output");
  expect(body.text()).toContain("Reference <caption>");
  expect(body.text()).toContain("original.png · input bytes were not stored in this run");
  expect(body.elements("img")).toHaveLength(1);
  expect(Reflect.get(body.elements("img")[0], "src")).toBe(`https://assets.example.test/recording/artifacts/${file.digest}.png`);
  expect(body.elements("a")).toHaveLength(1);
  expect(Reflect.get(body.elements("a")[0], "href")).toBe(`https://assets.example.test/recording/artifacts/${file.digest}.png`);
});

test("published pending, absent, unavailable and withheld values keep their state labels", () => {
  for (const [state, expected] of [["pending", "Waiting for an upstream result"], ["absent", "Absent"], ["unavailable", "Value unavailable"], ["withheld", "Withheld by the author"]] as const) {
    const body = inspectValue({ state });
    expect(body.text()).toContain(expected);
    expect(body.elements("img")).toEqual([]);
    expect(body.elements("pre")).toEqual([]);
  }
  expect(inspectValue({ data: { object: { redacted: { withheld: true }, legacy: { none: true } } } }).text()).toContain("Withheld by the author");
});
