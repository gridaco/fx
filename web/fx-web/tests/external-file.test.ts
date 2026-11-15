import { expect, test } from "bun:test";
import { ViewerProtocol } from "../../../js/fx/src/viewer-protocol";
import { ExternalFileController } from "../src/external-file";

const target: ViewerProtocol.FileTarget = { kind: "artifact", artifact: {
  digest: "a".repeat(64), url: "/api/artifacts/" + "a".repeat(64), name: "image.png", kind: "image/png", size: 10,
} };
const context = (id = "acme"): ViewerProtocol.Context => ({ subject: { kind: "run", workflow: { id } }, scope_id: null, selection: null, sampled_at: 0 });
async function tasks() { for (let n = 0; n < 8; n++) await Promise.resolve(); }

test("standalone and modifier activations retain ordinary browser navigation", async () => {
  let reads = 0, calls = 0, prevented = 0;
  const files = new ExternalFileController(() => { reads++; return context(); });
  expect(files.activate(target, { preventDefault: () => { prevented++; } })).toBeFalse();
  const detach = files.handleExternalFile(() => { calls++; });
  for (const modifiers of [{ button: 1 }, { ctrlKey: true }, { metaKey: true }, { altKey: true }, { shiftKey: true }, { defaultPrevented: true }]) {
    expect(files.activate(target, { ...modifiers, preventDefault: () => { prevented++; } })).toBeFalse();
  }
  await tasks();
  expect({ reads, calls, prevented }).toEqual({ reads: 0, calls: 0, prevented: 0 });
  detach();
  files.dispose();
});

test("the shared hook prevents default synchronously and captures the exact subject and target", async () => {
  let current = context("first"), prevented = false;
  const files = new ExternalFileController(() => current);
  const seen: ViewerProtocol.FileReference[] = [];
  const detach = files.handleExternalFile((reference) => { seen.push(reference); });
  const artifact = structuredClone(target);
  expect(files.activate(artifact, { button: 0, preventDefault: () => { prevented = true; } })).toBeTrue();
  expect(prevented).toBeTrue();
  expect(files.getSnapshot().pending).toBe(1);
  current = context("second");
  if (artifact.kind === "artifact") artifact.artifact.name = "changed.png";
  await tasks();
  expect(seen).toEqual([{ subject: context("first").subject, target }]);
  expect(files.getSnapshot()).toMatchObject({ available: true, pending: 0, error: null });
  detach();
  files.dispose();
});

test("one owner and token-bound cleanup work independently of the page transport", async () => {
  const files = new ExternalFileController(context);
  let first = 0, second = 0;
  const old = files.handleExternalFile(() => { first++; });
  expect(() => files.handleExternalFile(() => {})).toThrow("already registered");
  old();
  const next = files.handleExternalFile(() => { second++; });
  old();
  files.activate({ kind: "source", source: "nodes/acme.py", symbol: "render" });
  files.activate(target);
  await tasks();
  expect({ first, second }).toEqual({ first: 0, second: 2 });
  next();
  expect(files.getSnapshot().available).toBeFalse();
  files.dispose();
});

test("failures expose safe text and the captured fallback reference, never raw host errors", async () => {
  const files = new ExternalFileController(context);
  files.handleExternalFile((reference) => {
    if (reference.target.kind === "artifact") reference.target.artifact.url = "file:///private/workspace/secret.env";
    throw new Error("/private/workspace/secret.env token=private");
  });
  files.activate(target);
  await tasks();
  expect(files.getSnapshot()).toMatchObject({ pending: 0, error: "The host could not handle this file.", failedReference: { target } });
  expect(JSON.stringify(files.getSnapshot())).not.toContain("secret");
  files.clearError();
  expect(files.getSnapshot()).toMatchObject({ error: null, failedReference: null });
  files.dispose();
});

test("lost confirmations never replay a handler or undo a timeout on a late completion", async () => {
  const files = new ExternalFileController(context, { timeoutMs: 5 });
  let finish!: () => void, calls = 0;
  files.handleExternalFile(() => { calls++; return new Promise<void>((resolve) => { finish = resolve; }); });
  files.activate(target);
  await Bun.sleep(15);
  expect(calls).toBe(1);
  expect(files.getSnapshot()).toMatchObject({ pending: 0, error: "File handling was not confirmed before the timeout.", failedReference: { target } });
  finish();
  await tasks();
  expect(calls).toBe(1);
  expect(files.getSnapshot().error).not.toBeNull();
  files.dispose();
});

test("detachment and disposal cancel local waits and cannot invoke queued old handlers", async () => {
  const files = new ExternalFileController(context);
  let calls = 0;
  const detach = files.handleExternalFile(() => { calls++; });
  files.activate(target);
  detach();
  await tasks();
  expect(calls).toBe(0);
  expect(files.getSnapshot()).toMatchObject({ available: false, pending: 0, error: "File handling was not confirmed." });
  files.handleExternalFile(() => { calls++; return new Promise<void>(() => {}); });
  files.activate(target);
  await tasks();
  files.dispose();
  expect(files.getSnapshot()).toEqual({ available: false, pending: 0, error: null, failedReference: null });
  expect(files.activate(target)).toBeFalse();
});

test("invalid or unavailable references stay owned and never fall through to navigation", async () => {
  const files = new ExternalFileController(() => { throw new Error("private"); });
  files.handleExternalFile(() => { throw new Error("must not run"); });
  let prevented = 0;
  expect(files.activate(target, { preventDefault: () => { prevented++; } })).toBeTrue();
  expect(prevented).toBe(1);
  expect(files.getSnapshot().error).toBe("This file is unavailable for external handling.");
  files.dispose();
});
