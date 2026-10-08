import { describe, expect, test } from "bun:test";
import {
  cancellationRequested, ObservationError, ViewerController, parseRunEventBatch, parseRunSnapshot,
  parseViewerSnapshot, readObservation, type RunEvent, type RunEventBatch,
  type RunObservationReader, type ViewerPollingScheduler, type ViewerRun, type ViewerSnapshot,
} from "../src/index";
import { graph } from "./graph.test";
import { nestedPlan } from "./scopes.test";

const event: RunEvent = { kind: "fx-run-events-v1", event: "run_started", invocation_id: "invocation-a", plan: "a".repeat(64), offset_ms: 0 };
function run(state = "running"): ViewerRun {
  return {
    kind: "fx-viewer-run-v1", workflow: { id: "sample", title: "Sample" }, run_name: "sample-run",
    state, stand_in: false, charged_usd: null, estimate: null, inputs: {}, outputs: {}, artifacts: [], warnings: [],
    nodes: [{ id: "read#1", path: "read", title: "Read", uses: null, state, reads: [], with: {}, outputs: {}, cache: null, error: null, duration_ms: null }],
  };
}
function snapshot(cursor = "cursor-a", state = "running"): ViewerSnapshot {
  return { kind: "fx-run-snapshot-v1", cursor, plan: graph(), events: [event], view: run(state) };
}
function batch(cursor = "cursor-a", events: RunEvent[] = []): RunEventBatch {
  return { kind: "fx-run-event-batch-v1", cursor, events, has_more: false };
}

class Clock implements ViewerPollingScheduler {
  private serial = 0;
  readonly jobs = new Map<number, { callback: () => void; delay: number }>();
  schedule(callback: () => void, delay: number) { const id = ++this.serial; this.jobs.set(id, { callback, delay }); return id; }
  cancel(handle: unknown) { this.jobs.delete(handle as number); }
  get delay() { return this.jobs.values().next().value?.delay; }
  async tick() {
    const entry = this.jobs.entries().next().value;
    if (!entry) throw new Error("No scheduled poll");
    this.jobs.delete(entry[0]); entry[1].callback();
    await Bun.sleep(0);
  }
}

describe("observation read boundary", () => {
  test("accepts additive fields and unfamiliar events without interpreting their payloads", () => {
    const input = { ...snapshot(), future: true, events: [{ ...event, event: "future_event", payload: { nested: 1 } }] };
    expect(parseViewerSnapshot(input)).toEqual(input);
    const generic = { ...input } as Record<string, unknown>; delete generic.view;
    expect(parseRunSnapshot(generic).events[0].event).toBe("future_event");
    expect(() => parseViewerSnapshot(generic)).toThrow();
    expect(parseRunEventBatch({ ...batch(), future: true }).has_more).toBeFalse();
  });
  test("refuses unsupported versions and missing envelope fields", () => {
    expect(() => parseRunSnapshot({ ...snapshot(), kind: "fx-run-snapshot-v2" })).toThrow("does not support");
    expect(() => parseRunEventBatch({ ...batch(), kind: "fx-run-event-batch-v2" })).toThrow("does not support");
    expect(() => parseRunSnapshot({ ...snapshot(), cursor: "" })).toThrow("malformed");
    expect(() => parseRunEventBatch({ ...batch(), has_more: undefined })).toThrow("malformed");
    for (const field of ["kind", "event", "invocation_id", "plan", "offset_ms"]) {
      const incomplete = { ...event } as Record<string, unknown>; delete incomplete[field];
      expect(() => parseRunEventBatch({ ...batch(), events: [incomplete] })).toThrow("malformed");
    }
    expect(() => parseRunSnapshot({ ...snapshot(), events: [{ ...event, offset_ms: -1 }] })).toThrow("malformed");
    expect(() => parseRunSnapshot({ ...snapshot(), events: [{ ...event, event: "" }] })).toThrow("malformed");
    expect(() => parseRunSnapshot({ ...snapshot(), events: [{ ...event, invocation_id: "" }] })).toThrow("malformed");
    expect(() => parseRunSnapshot({ ...snapshot(), plan: {} })).toThrow();
  });
  test("uses escaped cursor parameters, no-store reads, and structured reconnect errors", async () => {
    const original = globalThis.fetch;
    const requests: { url: string; options?: RequestInit }[] = [];
    let conflict = false;
    globalThis.fetch = (async (url: string | URL | Request, options?: RequestInit) => {
      requests.push({ url: String(url), options });
      return new Response(JSON.stringify(conflict
        ? { kind: "fx-run-observation-error-v1", code: "run_changed", message: "The selected run changed." }
        : batch()), { status: conflict ? 409 : 200 });
    }) as typeof fetch;
    try {
      const signal = new AbortController().signal;
      await readObservation.events("cursor&?/+", signal);
      expect(new URL(requests[0].url, "http://localhost").searchParams.get("after")).toBe("cursor&?/+");
      expect(new URL(requests[0].url, "http://localhost").searchParams.get("limit")).toBe("256");
      expect(requests[0].options).toMatchObject({ cache: "no-store", signal });
      conflict = true;
      try { await readObservation.events("old", signal); throw new Error("Expected a conflict"); }
      catch (error) { expect(error).toBeInstanceOf(ObservationError); expect((error as ObservationError).requiresSnapshot).toBeTrue(); }
    } finally { globalThis.fetch = original; }
  });
});

describe("serial live viewer observation", () => {
  test("recorded cancellation intent clears on terminal or resume without changing run state", async () => {
    const requested = { ...event, event: "cancel_requested", source: "cli" };
    const terminal = { ...event, event: "run_cancelled" };
    const resumed = { ...event, invocation_id: "invocation-b" };
    expect(cancellationRequested([event, requested])).toBeTrue();
    expect(cancellationRequested([event, requested, terminal])).toBeFalse();
    expect(cancellationRequested([event, requested, resumed, requested])).toBeFalse();
    const clock = new Clock();
    let current = { ...snapshot(), view: run("unfinished"), events: [event, requested] };
    const observation: RunObservationReader = {
      async snapshot() { return current; },
      async events() { return batch(current.cursor, current.events); },
    };
    const controller = new ViewerController(async () => run(), { observation, scheduler: clock });
    await controller.refresh(); controller.select("instance:read#1");
    expect(controller.getSnapshot()).toMatchObject({ cancelling: true, view: { state: "unfinished" } });
    current = { ...snapshot("terminal"), view: run("cancelled"), events: [event, requested, terminal] };
    await clock.tick();
    expect(controller.getSnapshot()).toMatchObject({ cancelling: false, selected: "instance:read#1", view: { state: "cancelled" } });
    current = { ...snapshot("resumed"), view: run("unfinished"), events: [event, requested, terminal, resumed] };
    await clock.tick();
    expect(controller.getSnapshot()).toMatchObject({ cancelling: false, view: { state: "unfinished" } });
    controller.dispose();
  });
  test("live projections retain nested navigation, breadcrumbs and selected instances", async () => {
    const clock = new Clock(), plan = nestedPlan();
    let state = "running";
    const observation: RunObservationReader = {
      async snapshot() {
        return { ...snapshot(), plan, view: { ...run(state), workflow: plan.workflow, scopes: plan.scopes,
          nodes: plan.instances.map((node) => ({ id: node.id, path: node.path, title: node.path, uses: node.uses, state,
            reads: node.reads, with: {}, outputs: {}, cache: null, error: null, duration_ms: null,
            ports: plan.types?.[node.uses].ports, bindings: node.bindings, interface_bindings: node.interface_bindings, needs: node.needs, judges: node.judges })) } };
      },
      async events() { return batch("later", [event]); },
    };
    const controller = new ViewerController(async () => run(), { observation, scheduler: clock });
    await controller.refresh(); controller.openScope("scope:batch.item#1"); controller.openScope("scope:batch.item.child#1.1"); controller.select("instance:deep.paint#1");
    const breadcrumbs = controller.getSnapshot().breadcrumbs;
    state = "succeeded"; await clock.tick();
    expect(controller.getSnapshot()).toMatchObject({ scope: "scope:batch.item.child#1.1", selected: "instance:deep.paint#1", breadcrumbs });
    expect(controller.getSnapshot().graph.nodes.find((node) => node.id === "instance:deep.paint#1")!.state).toBe("succeeded");
    controller.dispose();
  });
  test("unchanged polls avoid projections and completion still watches for resumed invocations", async () => {
    const clock = new Clock();
    let current = snapshot(), reads = 0, next = batch();
    const cursors: string[] = [];
    const observation: RunObservationReader = {
      async snapshot() { reads++; return current; },
      async events(after) { cursors.push(after); return next; },
    };
    const controller = new ViewerController(async () => run(), { observation, scheduler: clock });
    await controller.refresh();
    controller.select("instance:read#1");
    const originalGraph = controller.getSnapshot().graph, originalUpdated = controller.getSnapshot().updated;
    expect(clock.delay).toBe(1000);
    await clock.tick();
    expect(reads).toBe(1); expect(controller.getSnapshot().graph).toBe(originalGraph); expect(controller.getSnapshot().updated).toBe(originalUpdated);
    const loading: boolean[] = []; const unsubscribe = controller.subscribe(() => loading.push(controller.getSnapshot().loading));
    next = batch("cursor-b", [{ ...event, event: "run_finished" }]);
    current = snapshot("cursor-c", "succeeded");
    await clock.tick();
    expect(controller.getSnapshot()).toMatchObject({ selected: "instance:read#1", view: { state: "succeeded" } });
    expect(loading.every((value) => !value)).toBeTrue();
    expect(clock.delay).toBe(1000);
    next = batch("cursor-d", [{ ...event, invocation_id: "invocation-b" }]);
    current = snapshot("cursor-e", "running");
    await clock.tick();
    expect(cursors).toEqual(["cursor-a", "cursor-a", "cursor-c"]);
    expect(controller.getSnapshot().view).toMatchObject({ state: "running" });
    unsubscribe(); controller.dispose(); expect(clock.jobs.size).toBe(0);
  });
  test("requests do not overlap, and disposal aborts and ignores a late response", async () => {
    const clock = new Clock();
    let resolve!: (value: RunEventBatch) => void, signal!: AbortSignal;
    const observation: RunObservationReader = {
      async snapshot() { return snapshot(); },
      events(_after, requestSignal) { signal = requestSignal; return new Promise((done) => { resolve = done; }); },
    };
    const controller = new ViewerController(async () => run(), { observation, scheduler: clock });
    await controller.refresh(); await clock.tick();
    expect(clock.jobs.size).toBe(0); expect(signal.aborted).toBeFalse();
    const state = controller.getSnapshot(); controller.dispose(); expect(signal.aborted).toBeTrue();
    resolve(batch("later", [event])); await Bun.sleep(0);
    expect(controller.getSnapshot()).toBe(state); expect(clock.jobs.size).toBe(0);
  });
  test("cursor conflicts reattach a consistent snapshot while preserving selection", async () => {
    const clock = new Clock(); let reads = 0;
    const observation: RunObservationReader = {
      async snapshot() { return snapshot(`cursor-${++reads}`, reads === 1 ? "running" : "succeeded"); },
      async events() { throw new ObservationError("invalid_cursor", "The cursor is no longer valid.", 409); },
    };
    const controller = new ViewerController(async () => run(), { observation, scheduler: clock });
    await controller.refresh(); controller.select("instance:read#1"); await clock.tick();
    expect(reads).toBe(2); expect(controller.getSnapshot()).toMatchObject({ selected: "instance:read#1", error: null, view: { state: "succeeded" } });
    controller.dispose();
  });
  test("disconnects preserve the graph, retry with bounded backoff, and recover without a full read", async () => {
    const clock = new Clock(); let fail = true, reads = 0;
    const observation: RunObservationReader = {
      async snapshot() { reads++; return snapshot(); },
      async events() { if (fail) throw new Error("Disconnected"); return batch(); },
    };
    const controller = new ViewerController(async () => run(), { observation, scheduler: clock });
    await controller.refresh(); const originalGraph = controller.getSnapshot().graph;
    for (const delay of [2000, 4000, 8000, 8000]) {
      await clock.tick(); expect(clock.delay).toBe(delay);
      expect(controller.getSnapshot().graph).toBe(originalGraph); expect(controller.getSnapshot().error).toBe("Disconnected");
    }
    fail = false; await clock.tick();
    expect(clock.delay).toBe(1000); expect(controller.getSnapshot().error).toBeNull(); expect(reads).toBe(1);
    controller.dispose();
  });
  test("an initial snapshot failure retries attachment; static plans never poll", async () => {
    const clock = new Clock(); let reads = 0;
    const observation: RunObservationReader = {
      async snapshot() { if (++reads === 1) throw new Error("Disconnected"); return snapshot(); },
      async events() { return batch(); },
    };
    const controller = new ViewerController(async () => run(), { observation, scheduler: clock });
    await controller.refresh(); expect(controller.getSnapshot().error).toBe("Disconnected"); expect(clock.delay).toBe(2000);
    await clock.tick(); expect(controller.getSnapshot().view).toMatchObject({ state: "running" }); expect(clock.delay).toBe(1000); controller.dispose();
    const plan = new ViewerController(async () => graph(), { observation, scheduler: clock });
    await plan.refresh(); expect(clock.jobs.size).toBe(0); expect(reads).toBe(2); plan.dispose();
  });
  test("a manual refresh aborts an old poll and remains the only scheduled observer", async () => {
    const clock = new Clock(); let current = snapshot(), resolve!: (value: RunEventBatch) => void;
    const observation: RunObservationReader = {
      async snapshot() { return current; },
      events() { return new Promise((done) => { resolve = done; }); },
    };
    const controller = new ViewerController(async () => run(), { observation, scheduler: clock });
    await controller.refresh(); await clock.tick(); current = snapshot("new", "succeeded"); await controller.refresh();
    resolve(batch("old", [event])); await Bun.sleep(0);
    expect(controller.getSnapshot().view).toMatchObject({ state: "succeeded" }); expect(clock.jobs.size).toBe(1); controller.dispose();
  });
});
