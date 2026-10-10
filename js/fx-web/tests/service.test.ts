import { describe, expect, test } from "bun:test";
import {
  ViewerController, ViewerEntryController, canvasGraph, observationReader, parseServiceIndex, parseViewerEntry,
  parseViewerRoute, parseViewerRun, parseViewerSnapshot, validateViewerApiBase, viewerEntryUrl,
  type ServiceIndex, type ViewerPollingScheduler, type ViewerRun,
} from "../src/index";
import { graph } from "./graph.test";

const project = "a".repeat(64), id = "b".repeat(64), digest = "c".repeat(64);
const runUrl = viewerEntryUrl(project, "run", id);
const apiBase = `${runUrl}api`;
const route = parseViewerRoute(runUrl);

function catalog(state = "unfinished"): ServiceIndex {
  return { kind: "fx-service-index-v1", project_id: project, title: "Local project", entries: [
    { id, kind: "run", title: "Delayed colors", url: runUrl, state },
    { id, kind: "plan", title: "Color plan", url: viewerEntryUrl(project, "plan", id), state: "planned" },
  ] };
}

function run(base = apiBase): ViewerRun {
  return {
    kind: "fx-viewer-run-v1", workflow: { id: "sample", title: "Sample" }, run_name: "sample-run",
    state: "running", stand_in: false, charged_usd: null, estimate: null, inputs: {}, outputs: {}, warnings: [],
    nodes: [{ id: "read#1", path: "read", title: "Read", uses: null, state: "running", reads: [], with: {},
      outputs: { image: { file: { digest, kind: "image/png", name: "color.png", size: 12 } } }, cache: null, error: null, duration_ms: null }],
    artifacts: [{ digest, kind: "image/png", name: "color.png", size: 12, available: true, url: `${base}/artifacts/${digest}` }],
  };
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

describe("project route and artifact boundary", () => {
  test("resolves root, run, and plan paths without accepting arbitrary request prefixes", () => {
    expect(parseViewerRoute("/")).toEqual({ api_base: "/api", project_id: null, entry_id: null, kind: "root" });
    expect(route).toEqual({ api_base: apiBase, project_id: project, entry_id: id, kind: "run" });
    expect(parseViewerRoute(runUrl.slice(0, -1))).toEqual(route);
    expect(parseViewerRoute(viewerEntryUrl(project, "plan", id)).kind).toBe("plan");
    for (const path of ["", "//other.invalid/", `https://other.invalid${runUrl}`, `${runUrl}?other`,
      runUrl.replace(project, ".."), runUrl.replace(id, "%2e%2e"), runUrl.replace("/runs/", "/other/"),
      runUrl.replace(id, id.toUpperCase()), `${runUrl}extra/`, runUrl.replace("/runs/", "//runs/"),
      runUrl.replace(id, "folder%2fother")]) {
      expect(() => parseViewerRoute(path)).toThrow();
    }
    for (const base of ["https://example.invalid/api", "//example.invalid/api", `${apiBase}/../api`, `${apiBase}/`, "/api?x", "/p/a/runs/b/api"]) {
      expect(() => validateViewerApiBase(base)).toThrow();
    }
  });

  test("scoped artifacts stay within the selected entry in views, snapshots and canvas previews", () => {
    const recorded = run();
    expect(parseViewerRun(recorded, apiBase)).toEqual(recorded);
    expect(canvasGraph(recorded, null, apiBase).nodes[0].preview?.url).toBe(`${apiBase}/artifacts/${digest}`);
    expect(canvasGraph(recorded).nodes[0].preview).toBeUndefined();
    const snapshot = { kind: "fx-run-snapshot-v1", cursor: "cursor", plan: graph(), events: [], view: recorded };
    expect(parseViewerSnapshot(snapshot, apiBase).view).toEqual(recorded);
    expect(() => parseViewerSnapshot(snapshot)).toThrow();
    for (const url of [
      `/api/artifacts/${digest}`, `${apiBase.replace(project, "d".repeat(64))}/artifacts/${digest}`,
      `${apiBase.replace(id, "d".repeat(64))}/artifacts/${digest}`,
      `${apiBase.replace("/runs/", "/plans/")}/artifacts/${digest}`,
      `${apiBase}/artifacts/${id}`, `${apiBase}/artifacts/${digest}?download`,
      `http://127.0.0.1${apiBase}/artifacts/${digest}`, `//other.invalid${apiBase}/artifacts/${digest}`,
      `${apiBase}/artifacts/../${digest}`, `${apiBase}/artifacts/%2e%2e/${digest}`,
    ]) {
      const invalid = run(); invalid.artifacts[0].url = url;
      expect(() => parseViewerRun(invalid, apiBase)).toThrow();
      expect(canvasGraph(invalid, null, apiBase).nodes[0].preview).toBeUndefined();
    }
    expect(parseViewerRun(run("/api"))).toEqual(run("/api"));
  });

  test("catalog links must match their project, kind and entry identity", () => {
    expect(parseServiceIndex(catalog())).toEqual(catalog());
    for (const url of ["javascript:alert(1)", "https://example.invalid/", "/", runUrl.replace(project, id),
      runUrl.replace("/runs/", "/plans/"), runUrl.replace(id, digest), `${runUrl}?other`]) {
      const invalid = catalog(); invalid.entries[0].url = url;
      expect(() => parseServiceIndex(invalid)).toThrow("invalid entry");
    }
    const duplicate = catalog(); duplicate.entries.push(duplicate.entries[0]);
    expect(() => parseServiceIndex(duplicate)).toThrow("duplicate");
    expect(() => parseServiceIndex({ ...catalog(), project_id: "../escape" })).toThrow();
    expect(() => parseServiceIndex({ ...catalog(), entries: [{ ...catalog().entries[0], state: undefined }] })).toThrow();
    expect(() => parseServiceIndex({ ...catalog(), kind: "fx-service-index-v2" })).toThrow("does not support");
  });

  test("root discriminates service and standalone responses; deep pages enforce their entry kind", () => {
    const root = parseViewerRoute("/");
    expect(parseViewerEntry(catalog(), root).kind).toBe("fx-service-index-v1");
    expect(parseViewerEntry(run("/api"), root).kind).toBe("fx-viewer-run-v1");
    expect(parseViewerEntry(graph(), root).kind).toBe("fx-graph-v1");
    expect(parseViewerEntry(run(), route).kind).toBe("fx-viewer-run-v1");
    expect(() => parseViewerEntry(catalog(), route)).toThrow();
    expect(() => parseViewerEntry(graph(), route)).toThrow();
    expect(() => parseViewerEntry(run(), parseViewerRoute(viewerEntryUrl(project, "plan", id)))).toThrow();
  });

  test("observation requests use the selected API base with escaped cursors", async () => {
    const original = globalThis.fetch;
    const requests: { url: string; options?: RequestInit }[] = [];
    globalThis.fetch = (async (url: string | URL | Request, options?: RequestInit) => {
      requests.push({ url: String(url), options });
      return new Response(JSON.stringify(String(url).endsWith("/snapshot")
        ? { kind: "fx-run-snapshot-v1", cursor: "cursor", plan: graph(), events: [], view: run() }
        : { kind: "fx-run-event-batch-v1", cursor: "cursor", events: [], has_more: false }));
    }) as typeof fetch;
    try {
      const reader = observationReader(apiBase), signal = new AbortController().signal;
      await reader.snapshot(signal); await reader.events("cursor&?/+", signal);
      expect(requests[0].url).toBe(`${apiBase}/snapshot`);
      const events = new URL(requests[1].url, "http://localhost");
      expect(events.pathname).toBe(`${apiBase}/events`); expect(events.searchParams.get("after")).toBe("cursor&?/+");
      expect(requests.every(({ options }) => options?.cache === "no-store" && options.signal === signal)).toBeTrue();
    } finally { globalThis.fetch = original; }
  });
});

describe("project index lifecycle", () => {
  test("discovers catalog once, then polls its index; standalone and deep pages never poll the catalog", async () => {
    const clock = new Clock(), urls: string[] = [];
    let next = catalog();
    const controller = new ViewerEntryController("/", { scheduler: clock, reader: async (url) => { urls.push(url); return next; } });
    await controller.refresh(); const original = controller.getSnapshot().catalog;
    expect(controller.getSnapshot()).toMatchObject({ kind: "catalog", loading: false, error: null });
    expect(clock.delay).toBe(2000); await clock.tick();
    expect(controller.getSnapshot().catalog).toBe(original);
    next = catalog("succeeded"); await clock.tick();
    expect(controller.getSnapshot().catalog?.entries[0].state).toBe("succeeded");
    expect(urls).toEqual(["/api/view", "/api/catalog", "/api/catalog"]);
    controller.dispose(); expect(clock.jobs.size).toBe(0);
    for (const [path, value] of [["/", run("/api")], ["/", graph()], [runUrl, run()]] as const) {
      const entry = new ViewerEntryController(path, { scheduler: clock, reader: async () => value });
      await entry.refresh(); expect(entry.getSnapshot().kind).toBe("viewer"); expect(clock.jobs.size).toBe(0); entry.dispose();
    }
  });

  test("serial polls cancel on dispose and ignore late responses", async () => {
    const clock = new Clock(); let resolve!: (value: unknown) => void, signal!: AbortSignal, reads = 0;
    const controller = new ViewerEntryController("/", { scheduler: clock, reader: (_url, requestSignal) => {
      if (++reads === 1) return Promise.resolve(catalog());
      signal = requestSignal; return new Promise((done) => { resolve = done; });
    } });
    await controller.refresh(); await clock.tick(); expect(clock.jobs.size).toBe(0);
    const before = controller.getSnapshot(); controller.dispose(); expect(signal.aborted).toBeTrue();
    resolve(catalog("succeeded")); await Bun.sleep(0);
    expect(controller.getSnapshot()).toBe(before); expect(clock.jobs.size).toBe(0);
  });

  test("manual refresh owns the only request after replacing a slow poll", async () => {
    const clock = new Clock(); let resolve!: (value: unknown) => void, signal!: AbortSignal, reads = 0;
    const controller = new ViewerEntryController("/", { scheduler: clock, reader: (_url, requestSignal) => {
      if (++reads !== 2) return Promise.resolve(catalog(reads === 1 ? "unfinished" : "succeeded"));
      signal = requestSignal; return new Promise((done) => { resolve = done; });
    } });
    await controller.refresh(); await clock.tick(); await controller.refresh();
    expect(signal.aborted).toBeTrue(); resolve(catalog()); await Bun.sleep(0);
    expect(controller.getSnapshot().catalog?.entries[0].state).toBe("succeeded"); expect(clock.jobs.size).toBe(1); controller.dispose();
  });

  test("transient failures keep the catalog, back off, and clear errors on recovery", async () => {
    const clock = new Clock(); let failing = false;
    const controller = new ViewerEntryController("/", { scheduler: clock, reader: async () => {
      if (failing) throw new Error("Disconnected"); return catalog();
    } });
    await controller.refresh(); const before = controller.getSnapshot().catalog; failing = true;
    for (const delay of [4000, 8000, 8000]) {
      await clock.tick(); expect(clock.delay).toBe(delay); expect(controller.getSnapshot().catalog).toBe(before);
    }
    failing = false; await clock.tick(); expect(controller.getSnapshot().error).toBeNull(); expect(clock.delay).toBe(2000); controller.dispose();
  });

  test("a new project on the same port cannot silently replace the current catalog", async () => {
    const clock = new Clock(); let next = catalog();
    const controller = new ViewerEntryController("/", { scheduler: clock, reader: async () => next });
    await controller.refresh(); const original = controller.getSnapshot().catalog;
    next = { ...catalog(), project_id: digest, entries: [] }; await clock.tick();
    expect(controller.getSnapshot().catalog).toBe(original); expect(controller.getSnapshot().error).toContain("different project"); controller.dispose();
  });

  test("invalid paths make no requests; initial read errors allow retry", async () => {
    let reads = 0;
    const invalid = new ViewerEntryController("/unknown", { reader: async () => { reads++; return catalog(); } });
    await invalid.refresh(); expect(reads).toBe(0); expect(invalid.getSnapshot()).toMatchObject({ loading: false, error: expect.any(String) }); invalid.dispose();
    const clock = new Clock();
    const retry = new ViewerEntryController("/", { scheduler: clock, reader: async () => {
      if (++reads === 1) throw new Error("Offline"); return catalog();
    } });
    await retry.refresh(); expect(retry.getSnapshot().error).toBe("Offline"); expect(clock.jobs.size).toBe(0);
    await retry.refresh(); expect(retry.getSnapshot()).toMatchObject({ kind: "catalog", loading: false, error: null }); retry.dispose();
  });
});

describe("viewer controller API ownership", () => {
  test("one API base scopes default view, snapshot, events and canvas artifact URLs; the snapshot carries the layout", async () => {
    const original = globalThis.fetch;
    const requests: string[] = [];
    let base = apiBase;
    globalThis.fetch = (async (url: string | URL | Request) => {
      requests.push(String(url));
      const layout = { kind: "fx-layout-report-v1", file: null, revision: null, state: "none", cursor: "cursor&?/+", diagnostics: [], cells: {}, order: [] };
      const value = String(url).endsWith("/view") ? run(base)
        : String(url).endsWith("/snapshot") ? { kind: "fx-run-snapshot-v1", cursor: "cursor&?/+", plan: graph(), events: [], view: run(base), layout }
        : { kind: "fx-run-event-batch-v1", cursor: "cursor&?/+", events: [], has_more: false };
      return new Response(JSON.stringify(value));
    }) as typeof fetch;
    try {
      for (base of [apiBase, "/api"]) {
        requests.length = 0;
        const clock = new Clock(), controller = new ViewerController(undefined, { apiBase: base, scheduler: clock });
        try {
          await controller.refresh(); await clock.tick();
          expect(controller.getSnapshot().error).toBeNull();
          expect(requests.slice(0, 2)).toEqual([`${base}/view`, `${base}/snapshot`]);
          expect(controller.getSnapshot().layout?.kind).toBe("fx-layout-report-v1");
          const events = new URL(requests[2], "http://localhost");
          expect(events.pathname).toBe(`${base}/events`); expect(events.searchParams.get("after")).toBe("cursor&?/+");
          expect(controller.getSnapshot().graph.nodes[0].preview?.url).toBe(`${base}/artifacts/${digest}`);
        } finally { controller.dispose(); }
      }
    } finally { globalThis.fetch = original; }
  });

  test("a custom reader remains static unless observation is explicitly supplied", async () => {
    const clock = new Clock(); let reads = 0;
    const controller = new ViewerController(async () => { reads++; return run(); }, { apiBase, scheduler: clock });
    try {
      await controller.refresh();
      expect(reads).toBe(1); expect(clock.jobs.size).toBe(0);
      expect(controller.getSnapshot().error).toBeNull();
      expect(controller.getSnapshot().graph.nodes[0].preview?.url).toBe(`${apiBase}/artifacts/${digest}`);
    } finally { controller.dispose(); }
  });
});
