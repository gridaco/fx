import { describe, expect, test } from "bun:test";
import { ViewerProtocol as Protocol } from "../src/viewer-protocol.js";
import {
  ViewerClient,
  ViewerClientError,
  type ViewerContext,
  type ViewerMessageEvent,
  type ViewerSourceWindow,
  type ViewerTargetWindow,
} from "../src/viewer/client.js";

const ORIGIN = "http://127.0.0.1:8787";
const context = (sampled_at = 10): ViewerContext => ({
  subject: { kind: "run", workflow: { id: "acme", source: "workflows/acme.yaml" },
    reference: { kind: "service_entry", project_id: "a".repeat(64), entry_kind: "run", entry_id: "b".repeat(64) } },
  scope_id: null,
  selection: { kind: "instance", instance_id: "prepare#1", path: "prepare", step: "prepare", take: [1] },
  sampled_at,
});

class Source implements ViewerSourceWindow {
  readonly listeners = new Set<(event: ViewerMessageEvent) => void>();
  addEventListener(_type: "message", listener: (event: ViewerMessageEvent) => void) { this.listeners.add(listener); }
  removeEventListener(_type: "message", listener: (event: ViewerMessageEvent) => void) { this.listeners.delete(listener); }
  emit(data: unknown, source: unknown, origin = ORIGIN) {
    for (const listener of [...this.listeners]) listener({ data, source, origin });
  }
}
class Page implements ViewerTargetWindow {
  readonly requests: Protocol.Request[] = [];
  readonly origins: string[] = [];
  handle: ((request: Protocol.Request) => void) | undefined;
  constructor(readonly source: Source) {}
  postMessage(value: unknown, origin: string) {
    if (!Protocol.isRequest(value)) throw new Error("Invalid outgoing request");
    this.requests.push(value);
    this.origins.push(origin);
    this.handle?.(value);
  }
  reply(request: Protocol.Request, result: ViewerContext | null = context()) {
    this.source.emit({ channel: Protocol.CHANNEL, version: 1, type: "response", request_id: request.request_id, ok: true, result }, this);
  }
  fail(request: Protocol.Request, code = "not_ready") {
    this.source.emit({ channel: Protocol.CHANNEL, version: 1, type: "response", request_id: request.request_id,
      ok: false, error: { code, message: "The requested content is unavailable." } }, this);
  }
  update(value: ViewerContext) {
    this.source.emit({ channel: Protocol.CHANNEL, version: 1, type: "context_changed", context: value }, this);
  }
  last(): Protocol.Request { return this.requests.at(-1)!; }
  count(method: string) { return this.requests.filter((request) => request.method === method).length; }
}
function setup(timeoutMs = 1_000) {
  const source = new Source();
  const page = new Page(source);
  const client = new ViewerClient({ targetWindow: page, origin: ORIGIN, sourceWindow: source, timeoutMs });
  return { source, page, client };
}
async function microtasks() { for (let i = 0; i < 8; i++) await Promise.resolve(); }

describe("viewer protocol validation", () => {
  test("file references are portable and discriminate artifacts from editable sources", () => {
    const file = { subject: context().subject, target: { kind: "artifact", artifact: {
      digest: "c".repeat(64), url: "/api/artifacts/" + "c".repeat(64), name: "asset.png", kind: "image/png", size: 12,
    } } };
    expect(Protocol.isFileReference(file)).toBeTrue();
    expect(Protocol.isFileReference({ ...file, target: { ...file.target, artifact: { ...file.target.artifact, url: null } } })).toBeTrue();
    expect(Protocol.isFileReference({ subject: context().subject, target: { kind: "source", source: "nodes/acme.ts", symbol: "render", recorded_digest: "d".repeat(64) } })).toBeTrue();
    for (const source of ["/private/acme.py", "../acme.py", "nodes/../acme.py", "C:/acme.py", "file:acme.py", "nodes\\acme.py", "nodes//acme.py", "nodes/acme.py:render", "nodes/acme\u0085.py"]) {
      expect(Protocol.isFileTarget({ kind: "source", source })).toBeFalse();
    }
    for (const url of ["file:///private/asset.png", "javascript:alert(1)", "https://user:secret@assets.test/file.png", "https://assets.test/file.png?token=secret", "https://assets.test/file.png?", "https://assets.test/file.png#", "https://@assets.test/file.png", "//assets.test/file.png", "http:assets.test/file.png", "https:/assets.test/file.png", "https://assets.test/file name.png"]) {
      expect(Protocol.isFileTarget({ ...file.target, artifact: { ...file.target.artifact, url } })).toBeFalse();
    }
    expect(Protocol.isFileReference({ ...file, local_path: "/private/asset.png" })).toBeFalse();
    expect(Protocol.isFileTarget({ kind: "source", source: "nodes/acme.ts", local_path: "/private/acme.ts" })).toBeFalse();
    expect(Protocol.isFileReference({ ...file, subject: { kind: "catalog" } })).toBeFalse();
    expect(Protocol.isFileTarget({ ...file.target, artifact: { ...file.target.artifact, name: "asset\u0085.png" } })).toBeFalse();
    expect(Protocol.isFileTarget({ ...file.target, artifact: { ...file.target.artifact, kind: "image/\u0085png" } })).toBeFalse();
  });
  test("unknown methods and versions remain recognizable for explicit refusal", () => {
    const request = { channel: "fx_viewer", version: 2, type: "request", request_id: "r1", method: "new_method", params: {} };
    expect(Protocol.isRequest(request)).toBe(true);
    expect(Protocol.isMethod(request.method)).toBe(false);
    expect(Protocol.isParams(request.method, request.params)).toBe(false);
    expect(Protocol.isRequest({ ...request, request_id: "" })).toBe(false);
    expect(Protocol.isRequest({ ...request, params: [] })).toBe(false);
  });

  test("context distinguishes recorded instance, scope, pending repeat and boundary", () => {
    for (const selection of [
      context().selection,
      { kind: "scope", scope_id: "nested#1" },
      { kind: "pending_repeat", path: "batch" },
      { kind: "scope_boundary", scope_id: "nested#1", side: "input" },
      null,
    ]) expect(Protocol.isContext({ ...context(), selection })).toBe(true);
    expect(Protocol.isContext({ ...context(), subject: { kind: "run", reference: { kind: "export", manifest_sha256: "c".repeat(64) } } })).toBe(true);
    expect(Protocol.isContext({ ...context(), subject: { kind: "catalog" }, selection: null })).toBe(true);
  });

  test("malformed identities and private source paths are refused before reaching consumers", () => {
    for (const source of ["/private/project/fx.yaml", "C:/private/fx.yaml", "file:secret", "https://example.test/fx.yaml", "workflows\\acme.yaml", "bad\nfile"]) {
      expect(Protocol.isContext({ ...context(), subject: { kind: "run", workflow: { id: "acme", source } } })).toBe(false);
    }
    expect(Protocol.isContext({ ...context(), selection: { kind: "instance", instance_id: "" } })).toBe(false);
    expect(Protocol.isContext({ ...context(), selection: { kind: "instance", instance_id: "prepare#1", take: [0] } })).toBe(false);
    expect(Protocol.isContext({ ...context(), selection: { kind: "instance", instance_id: "prepare\n#1" } })).toBe(false);
    expect(Protocol.isContext({ ...context(), subject: { kind: "plan", reference: context().subject.reference } })).toBe(false);
    expect(Protocol.isContext({ ...context(), sampled_at: NaN })).toBe(false);
    expect(Protocol.isContext({ ...context(), retained: false })).toBe(false);
    expect(Protocol.isContext({ ...context(), subject: { kind: "run", reference: { kind: "export", manifest_sha256: "not-a-digest" } } })).toBe(false);
  });

  test("known methods validate only their own action arguments", () => {
    expect(Protocol.isParams("present", { action: "open_scope", scope_id: null })).toBe(true);
    expect(Protocol.isParams("present", { action: "reveal_instance", instance_id: "prepare#1" })).toBe(true);
    expect(Protocol.isParams("get_context", {})).toBe(true);
    expect(Protocol.isParams("get_context", { action: "fit" })).toBe(false);
    expect(Protocol.isAction({ action: "fit", instance_id: "prepare#1" })).toBe(false);
    expect(Protocol.isAction({ action: "run", target: "acme" })).toBe(false);
    expect(Protocol.isAction({ action: "open_scope" })).toBe(false);
  });
});

describe("external file delegation", () => {
  const reference: Protocol.FileReference = { subject: context().subject,
    target: { kind: "artifact", artifact: { digest: "c".repeat(64), url: "/api/artifacts/" + "c".repeat(64), name: "asset.png", kind: "image/png", size: 12 } } };
  const activation = (handler_id: string, activation_id = 1) => ({ channel: Protocol.CHANNEL, version: 1,
    type: "external_file", handler_id, activation_id, reference });

  test("one registration receives each activation once and reports only safe completion", async () => {
    const { client, page, source } = setup();
    page.handle = (request) => page.reply(request, null);
    const seen: Protocol.FileReference[] = [];
    const off = await client.handleExternalFile((file) => { seen.push(file); });
    const registration = page.requests.find((request) => request.method === "register_external_file_handler")!;
    const id = registration.params.handler_id as string;
    source.emit(activation(id), {}, ORIGIN);
    source.emit(activation(id), page, "https://other.test");
    source.emit(activation("someone-else"), page);
    source.emit({ ...activation(id), reference: { ...reference, local_path: "/private/secret" } }, page);
    source.emit(activation(id), page);
    source.emit(activation(id), page);
    await microtasks();
    expect(seen).toEqual([reference]);
    expect(page.count("external_file_result")).toBe(1);
    expect(page.requests.find((request) => request.method === "external_file_result")?.params).toEqual({ handler_id: id, activation_id: 1, ok: true });
    off();
    off();
    source.emit(activation(id, 2), page);
    await microtasks();
    expect(seen).toHaveLength(1);
    expect(page.count("unregister_external_file_handler")).toBe(1);
    client.dispose();
  });

  test("exceptions never send private host diagnostics and activations are never retried", async () => {
    const { client, page, source } = setup();
    page.handle = (request) => page.reply(request, null);
    const off = await client.handleExternalFile(() => { throw new Error("secret-key /private/workspace/image.png"); });
    const id = page.requests[0]!.params.handler_id as string;
    source.emit(activation(id), page);
    await microtasks();
    expect(page.requests.find((request) => request.method === "external_file_result")?.params).toEqual({ handler_id: id, activation_id: 1, ok: false });
    expect(JSON.stringify(page.requests)).not.toContain("secret-key");
    source.emit(activation(id), page);
    await microtasks();
    expect(page.count("external_file_result")).toBe(1);
    off();
    client.dispose();
  });

  test("registration refusal and timeout clean up without silently taking over or retrying", async () => {
    const { client, page } = setup(5);
    const lost = client.handleExternalFile(() => {});
    await expect(client.handleExternalFile(() => {})).rejects.toMatchObject({ code: "unavailable" });
    await expect(lost).rejects.toMatchObject({ code: "timeout" });
    expect(page.count("register_external_file_handler")).toBe(1);
    expect(page.count("unregister_external_file_handler")).toBe(1);
    page.handle = (request) => { if (request.method === "register_external_file_handler") page.fail(request, "unsupported_method"); };
    await expect(client.handleExternalFile(() => {})).rejects.toMatchObject({ code: "unsupported_method" });
    expect(page.count("register_external_file_handler")).toBe(2);
    client.dispose();
  });

  test("old detach and replayed activations cannot affect a new registration", async () => {
    const { client, page, source } = setup();
    page.handle = (request) => page.reply(request, null);
    let first = 0, next = 0;
    const oldOff = await client.handleExternalFile(() => { first++; });
    const oldId = page.last().params.handler_id as string;
    oldOff();
    const newOff = await client.handleExternalFile(() => { next++; });
    const newId = page.last().params.handler_id as string;
    oldOff();
    source.emit(activation(oldId, 99), page);
    source.emit(activation(newId), page);
    await microtasks();
    expect({ first, next }).toEqual({ first: 0, next: 1 });
    newOff();
    client.dispose();
  });

  test("disposal releases file ownership and prevents queued callbacks", async () => {
    const { client, page, source } = setup();
    page.handle = (request) => page.reply(request, null);
    let calls = 0;
    await client.handleExternalFile(() => { calls++; });
    const id = page.last().params.handler_id as string;
    source.emit(activation(id), page);
    client.dispose();
    await microtasks();
    expect(calls).toBe(0);
    expect(page.count("unregister_external_file_handler")).toBe(1);
    expect(page.count("external_file_result")).toBe(0);
    expect(source.listeners.size).toBe(0);
  });
});

describe("browser viewer client", () => {
  test("a request uses exact origin and accepts only its target, origin, channel and reply ID", async () => {
    const { client, page, source } = setup();
    const reply = client.getContext();
    const request = page.last();
    const message = { channel: "fx_viewer", version: 1, type: "response", request_id: request.request_id, ok: true, result: context(20) };
    source.emit(message, {}, ORIGIN);
    source.emit(message, page, "https://other.test");
    source.emit({ ...message, channel: "other" }, page);
    source.emit({ ...message, request_id: "other" }, page);
    let settled = false;
    void reply.then(() => { settled = true; });
    await microtasks();
    expect(settled).toBe(false);
    page.reply(request, context(30));
    expect(await reply).toEqual(context(30));
    expect(page.origins).toEqual([ORIGIN]);
    client.dispose();
  });

  test("two windows sharing a message source cannot answer each other's calls", async () => {
    const source = new Source();
    const a = new Page(source);
    const b = new Page(source);
    const first = new ViewerClient({ targetWindow: a, sourceWindow: source, origin: ORIGIN });
    const second = new ViewerClient({ targetWindow: b, sourceWindow: source, origin: ORIGIN });
    const one = first.getContext();
    const two = second.getContext();
    expect(a.last().request_id).not.toBe(b.last().request_id);
    b.reply(a.last(), context(99));
    a.reply(b.last(), context(98));
    a.reply(a.last(), context(1));
    b.reply(b.last(), context(2));
    expect(await one).toEqual(context(1));
    expect(await two).toEqual(context(2));
    first.dispose();
    second.dispose();
    expect(source.listeners.size).toBe(0);
  });

  test("matching malformed replies fail explicitly instead of hanging", async () => {
    const { client, page, source } = setup();
    const result = client.getContext();
    const rejected = result.catch((error: unknown) => error);
    source.emit({ channel: "fx_viewer", version: 1, type: "response", request_id: page.last().request_id,
      ok: true, result: { ...context(), scope_id: 12 } }, page);
    expect(await rejected).toMatchObject({ code: "invalid_response" });
    client.dispose();
  });

  test("incompatible protocol and remote refusal remain typed errors", async () => {
    const { client, page, source } = setup();
    const incompatible = client.getContext();
    const rejected = incompatible.catch((error: unknown) => error);
    source.emit({ channel: "fx_viewer", version: 2, type: "response", request_id: page.last().request_id, ok: true, result: context() }, page);
    expect(await rejected).toMatchObject({ name: "ViewerClientError", code: "unsupported_version" });
    const refused = client.present({ action: "fit" });
    page.fail(page.last(), "unavailable");
    try { await refused; throw new Error("Expected refusal"); } catch (error) {
      expect(error).toBeInstanceOf(ViewerClientError);
      expect(error).toMatchObject({ code: "unavailable", message: "The requested content is unavailable." });
    }
    client.dispose();
  });

  test("presentation timeout leaves the outcome unconfirmed and never retries", async () => {
    const { client, page } = setup(10);
    await expect(client.present({ action: "reveal_instance", instance_id: "prepare#1" })).rejects.toMatchObject({ code: "timeout" });
    expect(page.count("present")).toBe(1);
    page.reply(page.requests[0]!);
    client.dispose();
    expect(page.count("present")).toBe(1);
  });

  test("concurrent local listeners share one wire subscription and retain separate snapshots", async () => {
    const { client, page } = setup();
    const a: ViewerContext[] = [];
    const b: ViewerContext[] = [];
    const first = client.subscribe((value) => { a.push(value); value.selection = null; });
    const second = client.subscribe((value) => b.push(value));
    expect(page.count("subscribe")).toBe(1);
    const subscribe = page.last();
    page.update(context(20));
    page.reply(subscribe, context(10));
    const [removeA, removeB] = await Promise.all([first, second]);
    expect(a.map((value) => value.sampled_at)).toEqual([20]);
    expect(b).toEqual([context(20)]);
    page.update(context(30));
    expect(b).toEqual([context(20), context(30)]);
    removeA();
    removeA();
    expect(page.count("unsubscribe")).toBe(0);
    page.update(context(40));
    expect(a.length).toBe(2);
    expect(b.at(-1)).toEqual(context(40));
    removeB();
    expect(page.count("unsubscribe")).toBe(1);
    page.reply(page.last(), null);
    await microtasks();
    client.dispose();
    expect(page.count("unsubscribe")).toBe(1);
  });

  test("listener exceptions do not prevent other listeners or cleanup", async () => {
    const { client, page } = setup();
    page.handle = (request) => page.reply(request, request.method === "unsubscribe" ? null : context());
    const seen: ViewerContext[] = [];
    const removeA = await client.subscribe(() => { throw new Error("Listener failed"); });
    const removeB = await client.subscribe((value) => seen.push(value));
    page.update(context(50));
    expect(seen).toEqual([context(), context(50)]);
    expect(page.count("subscribe")).toBe(1);
    removeA();
    removeB();
    await microtasks();
    client.dispose();
  });

  test("concurrent subscription refusal is shared and is not retried", async () => {
    const { client, page } = setup();
    const one = client.subscribe(() => {});
    const two = client.subscribe(() => {});
    const rejected = Promise.all([one.catch((error: unknown) => error), two.catch((error: unknown) => error)]);
    page.fail(page.last());
    for (const error of await rejected) expect(error).toMatchObject({ code: "not_ready" });
    expect(page.count("subscribe")).toBe(1);
    client.dispose();
  });

  test("new subscription waits for pending wire unsubscription", async () => {
    const { client, page } = setup();
    const first = client.subscribe(() => {});
    page.reply(page.last());
    const remove = await first;
    remove();
    const unsubscribe = page.last();
    const next = client.subscribe(() => {});
    expect(page.count("subscribe")).toBe(1);
    page.reply(unsubscribe, null);
    await microtasks();
    expect(page.count("subscribe")).toBe(2);
    page.reply(page.last(), context(60));
    const removeNext = await next;
    removeNext();
    page.reply(page.last(), null);
    await microtasks();
    client.dispose();
  });

  test("disposal settles all pending calls and removes local resources", async () => {
    const { client, page, source } = setup();
    const pending = client.getContext();
    const subscription = client.subscribe(() => {});
    const rejected = Promise.all([pending.catch((error: unknown) => error), subscription.catch((error: unknown) => error)]);
    client.dispose();
    for (const error of await rejected) expect(error).toMatchObject({ code: "disposed" });
    expect(source.listeners.size).toBe(0);
    expect(page.requests.every((request) => request.method !== "present")).toBe(true);
    expect(page.count("unsubscribe")).toBeGreaterThanOrEqual(1);
    const count = page.requests.length;
    client.dispose();
    expect(page.requests.length).toBe(count);
    await expect(client.getContext()).rejects.toMatchObject({ code: "disposed" });
  });

  test("origins must be exact HTTP(S) origins", () => {
    const source = new Source();
    const page = new Page(source);
    for (const origin of ["*", "null", "file:///viewer", "https://example.test/viewer", "https://user:secret@example.test", "https://example.test/"]) {
      expect(() => new ViewerClient({ targetWindow: page, sourceWindow: source, origin })).toThrow(ViewerClientError);
    }
    expect(source.listeners.size).toBe(0);
  });

  test("the client builds for browsers without importing the Node SDK or engine", async () => {
    const result = await Bun.build({ entrypoints: [new URL("../src/viewer/client.ts", import.meta.url).pathname], target: "browser" });
    expect(result.success).toBe(true);
    const output = await result.outputs[0]!.text();
    expect(output).not.toContain("node:");
    expect(output).not.toContain("GRIDA_FX_BIN");
    expect(output).not.toContain("host-entry");
  });
});

test("note-aware clients explicitly request note contexts while default clients keep v1 parameters", async () => {
  const source = new Source(), page = new Page(source);
  const client = new ViewerClient({ sourceWindow: source, targetWindow: page, origin: ORIGIN, includeNotes: true });
  const noteContext: ViewerContext = { ...context(), selection: { kind: "note", note_id: "note:root:guide", scope_id: null } };
  page.handle = (request) => page.reply(request, request.method === "unsubscribe" ? null : noteContext);
  expect((await client.getContext()).selection).toEqual(noteContext.selection);
  expect(page.last().params).toEqual({ include_notes: true });
  const release = await client.subscribe(() => {});
  expect(page.last().params).toEqual({ include_notes: true });
  await client.present({ action: "reveal_note", note_id: "note:root:guide" });
  expect(page.last().params).toEqual({ action: "reveal_note", note_id: "note:root:guide", include_notes: true });
  release(); client.dispose();
  const legacy = setup();
  legacy.page.handle = (request) => legacy.page.reply(request);
  await legacy.client.getContext();
  expect(legacy.page.last().params).toEqual({});
  const count = legacy.page.requests.length;
  await expect(legacy.client.present({ action: "reveal_note", note_id: "note:root:guide" })).rejects.toMatchObject({ code: "unsupported_method" });
  expect(legacy.page.requests).toHaveLength(count);
  legacy.client.dispose();
});

test("value inspection opts into portable references without exposing value content", async () => {
  const source = new Source(), page = new Page(source);
  const client = new ViewerClient({ sourceWindow: source, targetWindow: page, origin: ORIGIN, includeValues: true });
  const selected: ViewerContext = { ...context(), selection: { kind: "value", value_id: "value:root:input:prompt", scope_id: null } };
  page.handle = (request) => page.reply(request, request.method === "unsubscribe" ? null : selected);
  expect((await client.getContext()).selection).toEqual(selected.selection);
  expect(page.last().params).toEqual({ include_values: true });
  const release = await client.subscribe(() => {});
  expect(page.last().params).toEqual({ include_values: true });
  await client.present({ action: "reveal_value", value_id: "value:root:input:prompt" });
  expect(page.last().params).toEqual({ action: "reveal_value", value_id: "value:root:input:prompt", include_values: true });
  release(); client.dispose();
  const legacy = setup();
  const count = legacy.page.requests.length;
  await expect(legacy.client.present({ action: "reveal_value", value_id: "value:root:input:prompt" })).rejects.toMatchObject({ code: "unsupported_method" });
  expect(legacy.page.requests).toHaveLength(count);
  legacy.client.dispose();
  expect(Protocol.isParams("get_context", { include_values: false })).toBe(false);
  expect(Protocol.isParams("present", { action: "reveal_value", value_id: "v", include_notes: true, include_values: true })).toBe(true);
  expect(Protocol.isSelection({ ...selected.selection, data: "private content" })).toBe(false);
});
