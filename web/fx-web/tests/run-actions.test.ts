import { expect, test } from "bun:test";
import {
  parseRunActionReceipt, parseRunControl, parseViewerRoute, RunActionsController,
  type RunActionReceipt, type RunControl, type ViewerPollingScheduler,
} from "../src/index";

const project = "a".repeat(64);
const path = `/p/${project}/runs/${"b".repeat(64)}/`;
const nextPath = `/p/${project}/runs/${"e".repeat(64)}/`;
const route = parseViewerRoute(path);
const active: RunControl = {
  kind: "fx-viewer-run-control-v1", attempt_number: 1, invocation_id: "c".repeat(16),
  actions: { pause: true, cancel: true, resume: false, rerun: false },
};
const stopped: RunControl = { ...active, actions: { pause: false, cancel: false, resume: true, rerun: true } };
const accepted: RunActionReceipt = { kind: "fx-viewer-run-action-v1", outcome: "accepted", message: "The action was accepted." };

test("run action documents require a valid invocation and canonical exact same-project links", () => {
  expect(parseRunControl(active)).toEqual(active);
  expect(() => parseRunControl({ ...active, invocation_id: null })).toThrow();
  expect(() => parseRunControl({ ...active, actions: { pause: true } })).toThrow();
  expect(parseRunActionReceipt({ ...accepted, url: path }, "resume", route).url).toBe(path);
  expect(parseRunActionReceipt({ ...accepted, url: nextPath }, "rerun", route).url).toBe(nextPath);
  expect(() => parseRunActionReceipt(accepted, "rerun", route)).toThrow();
  expect(() => parseRunActionReceipt(accepted, "resume", route)).toThrow();
  for (const url of [path, `https://example.com${nextPath}`, nextPath.replace(project, "f".repeat(64)), nextPath.replace("/runs/", "/plans/"), `${nextPath}#review`]) {
    expect(() => parseRunActionReceipt({ ...accepted, url }, "rerun", route)).toThrow();
  }
  expect(() => parseRunActionReceipt({ ...accepted, url: nextPath }, "resume", route)).toThrow();
  expect(() => parseRunActionReceipt({ ...accepted, url: path }, "pause", route)).toThrow();
});

class Clock implements ViewerPollingScheduler {
  private work: (() => void) | null = null;
  schedule(callback: () => void) { this.work = callback; return callback; }
  cancel() { this.work = null; }
  async tick() {
    const work = this.work;
    this.work = null;
    if (!work) throw new Error("No control poll scheduled.");
    work();
    await Bun.sleep(0);
  }
}

test("a confirmation does not retarget a newer active invocation", async () => {
  let control = active;
  const clock = new Clock();
  const writes: RequestInit[] = [];
  const request = (async (_url: string, options: RequestInit = {}) => {
    if (options.method === "POST") writes.push(options);
    return Response.json(control);
  }) as typeof fetch;
  const controller = new RunActionsController(route, { request, scheduler: clock });
  controller.start();
  controller.setOpen(true);
  await Bun.sleep(0);
  controller.choose("cancel", null);
  expect(controller.getSnapshot().selection).toBeNull();
  controller.choose("cancel", active.invocation_id);
  expect(controller.getSnapshot().selection?.invocation_id).toBe(active.invocation_id!);
  await controller.submit("d".repeat(16));
  expect(writes).toEqual([]);
  control = { ...active, invocation_id: "d".repeat(16) };
  await clock.tick();
  await controller.submit(active.invocation_id);
  expect(writes).toEqual([]);
  expect(controller.getSnapshot().selection?.invocation_id).toBe(active.invocation_id!);
  controller.back();
  controller.choose("cancel", active.invocation_id);
  expect(controller.getSnapshot().selection).toBeNull();
  controller.dispose();
});

test("an uncertain start retries its original key and invocation without overlapping POSTs", async () => {
  let control = stopped;
  const calls: Array<{ url: string; options: RequestInit }> = [];
  let release!: () => void;
  const request = (async (url: string, options: RequestInit = {}) => {
    if (options.method !== "POST") return Response.json(control);
    calls.push({ url, options });
    if (calls.length === 1) {
      await new Promise<void>(resolve => { release = resolve; });
      throw new Error("Lost receipt");
    }
    return Response.json({ ...accepted, url: path });
  }) as typeof fetch;
  const controller = new RunActionsController(route, { request, requestId: () => "test-start" });
  controller.start();
  await Bun.sleep(0);
  controller.choose("resume", stopped.invocation_id);
  await controller.submit(stopped.invocation_id);
  expect(calls).toHaveLength(1);
  expect(controller.getSnapshot().submitting).toBeTrue();
  control = { ...active, invocation_id: "d".repeat(16) };
  release();
  await Bun.sleep(0);
  expect(controller.getSnapshot().receipt?.outcome).toBe("unknown");
  expect(controller.getSnapshot().control?.invocation_id).toBe(control.invocation_id!);
  await controller.submit(control.invocation_id);
  expect(calls.map(call => [call.url, call.options.headers, JSON.parse(String(call.options.body))])).toEqual(
    Array(2).fill([`${route.api_base}/control/resume`, { "Content-Type": "application/json", "X-FX-Action": "resume" },
      { invocation_id: stopped.invocation_id, request_id: "test-start" }]),
  );
  expect(controller.getSnapshot().receipt).toEqual({ ...accepted, url: path, action: "resume" });
  expect(controller.getSnapshot().selection).toBeNull();
  controller.dispose();
});

test("opening the panel waits for a fresh availability read", async () => {
  let reads = 0;
  let finishRead!: (response: Response) => void;
  const request = (async () => {
    reads += 1;
    if (reads === 1) return Response.json(active);
    return new Promise<Response>(resolve => { finishRead = resolve; });
  }) as typeof fetch;
  const controller = new RunActionsController(route, { request });
  controller.start();
  await Bun.sleep(0);
  controller.setOpen(true);
  expect(controller.getSnapshot().checking).toBeTrue();
  controller.choose("cancel", active.invocation_id);
  expect(controller.getSnapshot().selection).toBeNull();
  finishRead(Response.json(active));
  await Bun.sleep(0);
  expect(controller.getSnapshot().checking).toBeFalse();
  controller.choose("cancel", active.invocation_id);
  expect(controller.getSnapshot().selection?.action).toBe("cancel");
  controller.dispose();
});

test("portable viewers and saved plan pages never request controls", async () => {
  const calls: string[] = [];
  const request = (async (url: string) => {
    calls.push(url);
    return new Response(null, { status: 404 });
  }) as typeof fetch;
  for (const url of ["/", path.replace("/runs/", "/plans/"), path]) {
    const controller = new RunActionsController(parseViewerRoute(url), { request });
    controller.start();
    await Bun.sleep(0);
    expect(controller.getSnapshot().control).toBeNull();
    controller.dispose();
  }
  expect(calls).toEqual([`${route.api_base}/control`]);
});
