import { expect, test } from "bun:test";
import { ViewerProtocol } from "../../../js/fx/src/viewer-protocol";
import { ViewerClient } from "../../../js/fx/src/viewer/client";
import { ExportViewerController, ViewerController, ViewerPage, ViewerPresentation, parseRunExport, type ViewerRun } from "../src/index";
import { nestedPlan } from "./scopes.test";
import { graph } from "./graph.test";

async function harness(view = nestedPlan()) {
  const controller = new ViewerController(async () => view);
  await controller.refresh();
  const presentation = new ViewerPresentation({ getSnapshot: controller.getSnapshot, restore: (scope, selected) => controller.restore(scope, selected) });
  const painted: { scope: string | null; selected: string | null; focus: string | null }[] = [];
  presentation.attachCanvas((state, focus) => painted.push({ scope: state.scope, selected: state.selected, focus }));
  return { controller, presentation, painted };
}

test("presentation resolves nested recorded membership, including groups, and refuses without mutation", async () => {
  const view = nestedPlan();
  const before = JSON.stringify(view);
  const { controller, presentation, painted } = await harness(view);
  expect(presentation.present({ action: "reveal_instance", instance_id: "deep.paint#1" })).toMatchObject({
    scope_id: "scope:batch.item.child#1.1", selection: { kind: "instance", instance_id: "deep.paint#1" },
  });
  expect(painted.at(-1)).toEqual({ scope: "scope:batch.item.child#1.1", selected: "instance:deep.paint#1", focus: "instance:deep.paint#1" });
  expect(presentation.present({ action: "reveal_instance", instance_id: "a.paint#1" }).scope_id).toBe("scope:batch.item#1");
  const snapshot = controller.getSnapshot();
  for (const action of [
    { action: "reveal_instance", instance_id: "instance:a.paint#1" },
    { action: "open_scope", scope_id: "scope:batch#1" },
    { action: "open_scope", scope_id: "missing" },
  ] as const) expect(() => presentation.present(action)).toThrow();
  expect(controller.getSnapshot()).toBe(snapshot);
  expect(presentation.present({ action: "fit" }).selection).toMatchObject({ instance_id: "a.paint#1" });
  expect(painted.at(-1)?.focus).toBe("fit");
  expect(presentation.present({ action: "clear_selection" })).toMatchObject({ scope_id: "scope:batch.item#1", selection: null });
  expect(painted.at(-1)?.focus).toBeNull();
  expect(presentation.present({ action: "open_scope", scope_id: null })).toMatchObject({ scope_id: null, selection: null });
  expect(JSON.stringify(view)).toBe(before);
});

test("context distinguishes scopes, pending repeats, boundaries and recorded takes without values", async () => {
  const view = nestedPlan();
  view.workflow.file = "workflows/sample.yaml";
  view.instances[0].with = { secret_value: "not context" };
  const { controller, presentation } = await harness(view);
  controller.select("scope:batch.item#1");
  expect(presentation.getContext().selection).toEqual({ kind: "scope", scope_id: "scope:batch.item#1" });
  presentation.present({ action: "open_scope", scope_id: "scope:batch.item#1" });
  const boundary = controller.getSnapshot().graph.nodes.find((node) => node.kind === "boundary")!;
  controller.select(boundary.id);
  expect(presentation.getContext().selection).toEqual({ kind: "scope_boundary", scope_id: boundary.scope_id!, side: boundary.side! });
  const context = presentation.present({ action: "reveal_instance", instance_id: "seed#1" });
  expect(context.subject.workflow).toEqual({ id: view.workflow.id, source: "workflows/sample.yaml" });
  expect(context.selection).toMatchObject({ instance_id: "seed#1", take: [1] });
  expect(JSON.stringify(context)).not.toContain("secret_value");
  expect(JSON.stringify(context)).not.toContain("instance:seed#1");
  const pending = await harness(graph());
  pending.controller.select("pending:items");
  expect(pending.presentation.getContext().selection).toEqual({ kind: "pending_repeat", path: "items" });
  view.workflow.file = "/private/project.yaml";
  expect(presentation.getContext().subject.workflow?.source).toBeUndefined();
});

test("run context uses recorded fields and retained data remains explicit", async () => {
  const plan = graph();
  const run: ViewerRun = { kind: "fx-viewer-run-v1", workflow: plan.workflow, run_name: "recorded", state: "succeeded", stand_in: false,
    charged_usd: 0, estimate: null, inputs: {}, outputs: {}, artifacts: [], warnings: [],
    nodes: plan.instances.map((node) => ({ id: node.id, path: node.path, step: node.step, take: [2, 1], title: node.path,
      uses: node.uses, state: "succeeded", reads: [], with: {}, outputs: {}, cache: null, error: null, duration_ms: 0 })) };
  let fail = false;
  const controller = new ViewerController(async () => { if (fail) throw new Error("offline"); return run; });
  const presentation = new ViewerPresentation({ getSnapshot: controller.getSnapshot, restore: (scope, selected) => controller.restore(scope, selected) });
  expect(() => presentation.getContext()).toThrow("No workflow evidence");
  await controller.refresh();
  expect(() => presentation.present({ action: "fit" })).toThrow("not mounted");
  presentation.attachCanvas(() => {});
  const selected = presentation.present({ action: "reveal_instance", instance_id: "read#1" });
  expect(selected.subject).toEqual({ kind: "run", workflow: { id: plan.workflow.id } });
  expect(selected.selection).toMatchObject({ kind: "instance", instance_id: "read#1", take: [2, 1] });
  fail = true;
  await controller.refresh();
  expect(presentation.getContext().retained).toBeTrue();
});

test("the shared presentation projection can inspect a verified static export without disclosing values", async () => {
  const bytes = await Bun.file(new URL("../../../spec/vectors/exports/minimal.json", import.meta.url)).text();
  const manifest = parseRunExport(JSON.parse(bytes));
  const digest = new Bun.CryptoHasher("sha256").update(bytes).digest("hex");
  const controller = new ExportViewerController(async () => ({ manifest, manifest_url: "https://assets.example/export/manifest.json" }));
  await controller.load({ url: "https://assets.example/export/manifest.json", sha256: digest });
  const presentation = new ViewerPresentation({
    getSnapshot: () => { const state = controller.getSnapshot(); return { ...state, view: state.recording?.manifest.view ?? null }; },
    restore: (scope, selected) => controller.restore(scope, selected),
    reference: () => ({ kind: "export", manifest_sha256: digest }),
  });
  presentation.attachCanvas(() => {});
  const before = JSON.stringify(manifest);
  const result = presentation.present({ action: "reveal_instance", instance_id: manifest.view.nodes[0]!.id });
  expect(result.subject.reference).toEqual({ kind: "export", manifest_sha256: digest });
  expect(result.selection).toMatchObject({ kind: "instance", instance_id: manifest.view.nodes[0]!.id });
  const snapshot = controller.getSnapshot();
  expect(controller.restore(null, "absent")).toBeFalse();
  expect(controller.getSnapshot()).toBe(snapshot);
  expect(JSON.stringify(manifest)).toBe(before);
  controller.dispose();
});

class PageWindow {
  readonly window = this;
  closed = false;
  location: { origin: string };
  listeners = new Set<(event: MessageEvent) => void>();
  sent: { data: unknown; origin: string }[] = [];
  constructor(origin = "http://localhost:8787") { this.location = { origin }; }
  addEventListener(_type: string, listener: (event: MessageEvent) => void) { this.listeners.add(listener); }
  removeEventListener(_type: string, listener: (event: MessageEvent) => void) { this.listeners.delete(listener); }
  postMessage(data: unknown, origin: string) { this.sent.push({ data, origin }); }
  receive(source: PageWindow, data: unknown, origin = source.location.origin) {
    for (const listener of this.listeners) listener({ data, source, origin } as unknown as MessageEvent);
  }
}
const request = (method = "get_context", params: object = {}, version = 1) => ({
  channel: "fx_viewer", version, type: "request", request_id: "r", method, params,
});

async function endpoint() {
  const data = await harness();
  const window = new PageWindow();
  const peer = new PageWindow();
  const page = new ViewerPage(window as unknown as Window, {
    getContext: data.presentation.getContext, present: data.presentation.present, subscribe: data.controller.subscribe,
  });
  return { ...data, window, peer, page };
}

test("page endpoint rejects untrusted windows, pins grants, and returns safe protocol errors", async () => {
  const { window, peer, page } = await endpoint();
  window.receive(peer, request());
  expect(peer.sent.at(-1)).toMatchObject({ origin: window.location.origin, data: { ok: true, result: { subject: { kind: "plan" } } } });
  const foreign = new PageWindow("https://host.example");
  window.receive(foreign, request());
  expect(foreign.sent).toHaveLength(0);
  const revoke = page.allowHost(foreign as unknown as Window, foreign.location.origin);
  window.receive(foreign, request());
  expect(foreign.sent).toHaveLength(1);
  window.receive(new PageWindow(foreign.location.origin), request());
  window.receive(foreign, request(), "https://other.example");
  expect(foreign.sent).toHaveLength(1);
  revoke();
  window.receive(foreign, request());
  expect(foreign.sent).toHaveLength(1);
  for (const [message, code] of [
    [request("get_context", {}, 2), "unsupported_version"],
    [request("launch"), "unsupported_method"],
    [request("present", { action: "fit", extra: true }), "invalid_request"],
    [request("present", { action: "reveal_instance", instance_id: "absent" }), "not_found"],
  ] as const) {
    window.receive(peer, message);
    expect(peer.sent.at(-1)?.data).toMatchObject({ ok: false, error: { code } });
  }
  page.dispose();
  expect(window.listeners.size).toBe(0);
});

test("subscriptions coalesce user navigation and selection, repeat subscribe and unsubscribe are idempotent", async () => {
  const { window, peer, page, controller } = await endpoint();
  window.receive(peer, request("subscribe"));
  window.receive(peer, request("subscribe"));
  peer.sent.length = 0;
  controller.openScope("scope:batch.item#1");
  controller.select("instance:a.paint#1");
  await Promise.resolve();
  expect(peer.sent).toHaveLength(1);
  expect(peer.sent[0]?.data).toMatchObject({ type: "context_changed", context: { scope_id: "scope:batch.item#1", selection: { instance_id: "a.paint#1" } } });
  expect(ViewerProtocol.isContextEvent(peer.sent[0]?.data)).toBeTrue();
  controller.select("instance:a.paint#1");
  await Promise.resolve();
  expect(peer.sent).toHaveLength(1);
  window.receive(peer, request("unsubscribe"));
  window.receive(peer, request("unsubscribe"));
  peer.sent.length = 0;
  controller.select(null);
  await Promise.resolve();
  expect(peer.sent).toHaveLength(0);
  page.dispose();
});

test("real client and page endpoint agree on calls and subscriptions", async () => {
  const { window, peer, page, controller } = await endpoint();
  // Browser event.source is the other WindowProxy at each end.
  const target = { postMessage: (data: unknown) => window.receive(peer, data) };
  const listeners = new Set<(event: any) => void>();
  peer.postMessage = (data, origin) => { for (const listener of listeners) listener({ data, origin, source: target }); };
  const client = new ViewerClient({ targetWindow: target, origin: window.location.origin,
    sourceWindow: { addEventListener: (_, listener) => { listeners.add(listener); }, removeEventListener: (_, listener) => { listeners.delete(listener); } } });
  expect((await client.getContext()).subject.kind).toBe("plan");
  const contexts: ViewerProtocol.Context[] = [];
  const off = await client.subscribe((context) => contexts.push(context));
  controller.select("scope:batch.item#1");
  await Promise.resolve();
  expect(contexts.at(-1)?.selection).toEqual({ kind: "scope", scope_id: "scope:batch.item#1" });
  expect((await client.present({ action: "reveal_instance", instance_id: "deep.paint#1" })).selection).toMatchObject({ instance_id: "deep.paint#1" });
  off();
  client.dispose();
  page.dispose();
});
