import { expect, test } from "bun:test";
import { parseRunHistory, parseViewerRoute, RunHistoryController, type RunHistory, type ViewerPollingScheduler } from "../src/index";

const project = "a".repeat(64), run_id = "b".repeat(64), entry = "c".repeat(64), later = "d".repeat(64);
const prefix = `/p/${project}`;
const exact = `${prefix}/runs/${entry}/`;
const route = parseViewerRoute(exact);
const original: RunHistory = {
  kind: "fx-viewer-run-history-v1", run_id, latest_url: `${prefix}/run/${run_id}/`, latest_available: true, selected_attempt: 1,
  attempts: [{ attempt_number: 1, url: exact, state: "succeeded" }],
};
const withNewer: RunHistory = {
  ...original, attempts: [{ attempt_number: 2, url: `${prefix}/runs/${later}/`, state: "failed" }, ...original.attempts],
};

test("attempt history accepts only ordered, exact same-project links and the stable link for that run", () => {
  expect(parseRunHistory(withNewer, route)).toEqual(withNewer);
  expect(parseRunHistory({ ...original, latest_available: false }, route).latest_available).toBeFalse();
  for (const bad of [
    { ...withNewer, latest_available: undefined },
    { ...withNewer, latest_url: `https://example.com${withNewer.latest_url}` },
    { ...withNewer, latest_url: withNewer.latest_url.replace(run_id, later) },
    { ...withNewer, attempts: [...withNewer.attempts].reverse() },
    { ...withNewer, attempts: [...withNewer.attempts, original.attempts[0]] },
    { ...withNewer, selected_attempt: 2 },
    { ...withNewer, selected_attempt: 3 },
    ...[`${exact}#foo`, exact.slice(0, -1), `//example.com${exact}`, exact.replace("/runs/", "/plans/"), exact.replace(project, later)]
      .map(url => ({ ...original, attempts: [{ ...original.attempts[0], url }] })),
  ]) expect(() => parseRunHistory(bad, route)).toThrow();
  expect(() => parseRunHistory(original, parseViewerRoute("/"))).toThrow();
});

class Clock implements ViewerPollingScheduler {
  private job: (() => void) | null = null;
  schedule(callback: () => void) { this.job = callback; return callback; }
  cancel() { this.job = null; }
  async tick() {
    const job = this.job;
    this.job = null;
    if (!job) throw new Error("No scheduled poll");
    job();
    await Bun.sleep(0);
  }
  get scheduled() { return this.job !== null; }
}

test("a pinned attempt sees newer history through its own endpoint without changing the selection", async () => {
  const clock = new Clock(), routes: string[] = [];
  let value = original;
  const controller = new RunHistoryController(route, { scheduler: clock, reader: async (url) => {
    routes.push(url);
    return value;
  } });
  controller.start(); await Bun.sleep(0);
  expect(controller.getSnapshot().history).toEqual(original);
  expect(clock.scheduled).toBeFalse();
  controller.setOpen(true); await Bun.sleep(0);
  value = withNewer;
  await clock.tick();
  expect(controller.getSnapshot().history).toEqual(withNewer);
  expect(routes).toEqual(Array(3).fill(`${route.api_base}/history`));
  controller.setOpen(false);
  expect(clock.scheduled).toBeFalse();
  controller.dispose();
});

test("unsupported routes are skipped and unavailable endpoints leave history absent", async () => {
  const clock = new Clock(), routes: string[] = [];
  const options = { scheduler: clock, reader: async (url: string) => { routes.push(url); return null; } };
  for (const url of ["/", exact.replace("/runs/", "/plans/"), exact]) {
    const controller = new RunHistoryController(parseViewerRoute(url), options);
    controller.start(); await Bun.sleep(0);
    expect(controller.getSnapshot()).toEqual({ history: null, error: null });
    expect(clock.scheduled).toBeFalse();
    controller.dispose();
  }
  expect(routes).toEqual([`${route.api_base}/history`]);
});
