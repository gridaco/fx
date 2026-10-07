import { describe, expect, test } from "bun:test";
import { ArtifactController, CanvasViewport, ViewerController, type Artifact, type ViewerView } from "../src/index";
import { graph } from "./graph.test";

describe("vanilla canvas viewport", () => {
  test("restores an independent per-scope camera including a very small fitted zoom", () => {
    const viewport = new CanvasViewport();
    const snapshot = { x: -45, y: 23, zoom: 0.0008 };
    viewport.restore(snapshot);
    expect(viewport.getSnapshot()).toEqual(snapshot);
    snapshot.x = 900;
    expect(viewport.getSnapshot().x).toBe(-45);
    viewport.zoomAt(0.5, { x: 200, y: 100 });
    expect(viewport.getSnapshot().zoom).toBeCloseTo(0.0004);
    const valid = viewport.getSnapshot();
    for (const invalid of [{ ...valid, x: NaN }, { ...valid, y: Infinity }, { ...valid, zoom: 0 }, { ...valid, zoom: 4 }]) viewport.restore(invalid);
    expect(viewport.getSnapshot()).toEqual(valid);
  });
  test("fits bounds with room around the graph", () => {
    const viewport = new CanvasViewport();
    viewport.fit({ width: 1200, height: 600 }, { width: 800, height: 500 });
    const value = viewport.getSnapshot();
    expect(value.x).toBeGreaterThanOrEqual(32);
    expect(value.y).toBeGreaterThanOrEqual(32);
    expect(value.x + 1200 * value.zoom).toBeLessThanOrEqual(768);
    expect(value.y + 600 * value.zoom).toBeLessThanOrEqual(468);
  });
  test("pan changes only the viewport and snapshots are independent", () => {
    const viewport = new CanvasViewport();
    viewport.pan(30, -20);
    expect(viewport.getSnapshot()).toEqual({ x: 30, y: -20, zoom: 1 });
    const copy = viewport.getSnapshot();
    copy.x = 999;
    expect(viewport.getSnapshot().x).toBe(30);
  });
  test("two-finger scrolling pans both axes without changing zoom", () => {
    const viewport = new CanvasViewport();
    viewport.zoomAt(1.5, { x: 200, y: 100 });
    const before = viewport.getSnapshot();
    viewport.wheel({ deltaX: 24, deltaY: -36, deltaMode: 0, ctrlKey: false }, { x: 200, y: 100 }, { width: 800, height: 500 });
    expect(viewport.getSnapshot()).toEqual({ x: before.x - 24, y: before.y + 36, zoom: before.zoom });
  });
  test("pinch wheel events zoom around the pointer without applying pan deltas", () => {
    const viewport = new CanvasViewport();
    viewport.pan(30, 40);
    const point = { x: 210, y: 120 };
    const before = viewport.getSnapshot();
    viewport.wheel({ deltaX: 99, deltaY: -20, deltaMode: 0, ctrlKey: true }, point, { width: 800, height: 500 });
    const after = viewport.getSnapshot();
    expect(after.zoom).toBeGreaterThan(before.zoom);
    expect((point.x - after.x) / after.zoom).toBeCloseTo((point.x - before.x) / before.zoom);
    expect((point.y - after.y) / after.zoom).toBeCloseTo((point.y - before.y) / before.zoom);
    viewport.wheel({ deltaX: 0, deltaY: 20, deltaMode: 0, ctrlKey: true }, point, { width: 800, height: 500 });
    expect(viewport.getSnapshot().zoom).toBeCloseTo(before.zoom);
  });
  test("line and page wheel deltas use screen-space pan units", () => {
    const viewport = new CanvasViewport();
    const point = { x: 0, y: 0 };
    const size = { width: 800, height: 500 };
    viewport.wheel({ deltaX: 1, deltaY: -2, deltaMode: 1, ctrlKey: false }, point, size);
    expect(viewport.getSnapshot()).toEqual({ x: -16, y: 32, zoom: 1 });
    viewport.wheel({ deltaX: -1, deltaY: 1, deltaMode: 2, ctrlKey: false }, point, size);
    expect(viewport.getSnapshot()).toEqual({ x: 784, y: -468, zoom: 1 });
  });
  test("Fit includes a very wide graph instead of stopping at a fixed zoom floor", () => {
    const viewport = new CanvasViewport();
    viewport.fit({ width: 12000, height: 600 }, { width: 800, height: 500 });
    const fitted = viewport.getSnapshot();
    expect(fitted.zoom).toBeLessThan(0.1);
    expect(fitted.x).toBeGreaterThanOrEqual(32);
    expect(fitted.x + 12000 * fitted.zoom).toBeLessThanOrEqual(768);
    viewport.zoomAt(0.01, { x: 400, y: 250 });
    expect(viewport.getSnapshot().zoom).toBeLessThan(fitted.zoom);
    viewport.zoomAt(10, { x: 400, y: 250 });
    expect(viewport.getSnapshot().zoom).toBeCloseTo(fitted.zoom);
  });
  test("zoom keeps the content under the cursor fixed and respects bounds", () => {
    const viewport = new CanvasViewport();
    viewport.pan(30, 40);
    const point = { x: 210, y: 120 };
    const before = viewport.getSnapshot();
    viewport.zoomAt(1.5, point);
    const after = viewport.getSnapshot();
    expect((point.x - after.x) / after.zoom).toBeCloseTo((point.x - before.x) / before.zoom);
    expect((point.y - after.y) / after.zoom).toBeCloseTo((point.y - before.y) / before.zoom);
    viewport.zoomAt(1000, point);
    expect(viewport.getSnapshot().zoom).toBe(3);
    viewport.zoomAt(0.00001, point);
    expect(viewport.getSnapshot().zoom).toBe(0.01);
  });
});

describe("viewer controller", () => {
  test("selection is read-only, survives refresh, and ignores unknown IDs", async () => {
    const input = graph();
    const before = JSON.stringify(input);
    const controller = new ViewerController(async () => input);
    await controller.refresh();
    controller.select("instance:change#1");
    controller.select("not-a-node");
    expect(controller.getSnapshot().selected).toBe("instance:change#1");
    await controller.refresh();
    expect(controller.getSnapshot().selected).toBe("instance:change#1");
    controller.select("pending:items");
    expect(controller.getSnapshot().selected).toBe("pending:items");
    expect(JSON.stringify(input)).toBe(before);
  });
  test("a failed refresh preserves the last readable graph", async () => {
    let fail = false;
    const controller = new ViewerController(async () => { if (fail) throw new Error("Unavailable"); return graph(); });
    await controller.refresh();
    const view = controller.getSnapshot().view;
    fail = true;
    await controller.refresh();
    expect(controller.getSnapshot().view).toBe(view);
    expect(controller.getSnapshot().error).toBe("Unavailable");
    expect(controller.getSnapshot().loading).toBeFalse();
  });
  test("an old request cannot replace the newer graph", async () => {
    const pending: ((value: ViewerView) => void)[] = [];
    const controller = new ViewerController(() => new Promise((resolve) => pending.push(resolve)));
    const first = controller.refresh();
    const second = controller.refresh();
    const newer = graph();
    newer.workflow.title = "Newer";
    pending[1](newer);
    await second;
    pending[0](graph());
    await first;
    expect(controller.getSnapshot().view?.workflow.title).toBe("Newer");
  });
});

const textArtifact: Artifact = { digest: "a".repeat(64), name: "data.json", kind: "json", size: 20, available: true, url: `/api/artifacts/${"a".repeat(64)}` };

describe("artifact controller", () => {
  test("refresh retries a failed preview with the same digest", async () => {
    let attempts = 0;
    const controller = new ArtifactController(async () => { if (++attempts === 1) throw new Error("Unavailable"); return "{}"; });
    await controller.load(textArtifact);
    expect(controller.getSnapshot().error).toBeTrue();
    await controller.load({ ...textArtifact });
    expect(controller.getSnapshot()).toEqual({ error: false, loading: false, text: "{}" });
  });
  test("unavailable and oversized artifacts do not trigger fetches", async () => {
    let attempts = 0;
    const controller = new ArtifactController(async () => { attempts++; return "{}"; });
    await controller.load({ ...textArtifact, available: false, url: null });
    await controller.load({ ...textArtifact, size: 2 * 1024 * 1024 });
    expect(attempts).toBe(0);
  });
  test("cancelled artifact reads cannot overwrite refreshed text", async () => {
    const pending: ((value: string) => void)[] = [];
    const controller = new ArtifactController(() => new Promise((resolve) => pending.push(resolve)));
    const first = controller.load(textArtifact);
    const second = controller.load(textArtifact);
    pending[1]("new"); await second;
    pending[0]("old"); await first;
    expect(controller.getSnapshot().text).toBe("new");
  });
});
