import { expect, test } from "bun:test";
import { WorkflowCoverController } from "./workflow-cover-controller";

function fixture(reduced = false, refusePlayback = false) {
  const calls = { plays: 0, pauses: 0, loads: 0, disconnected: false };
  const video = {
    src: "", muted: false,
    play() { calls.plays++; return refusePlayback ? Promise.reject(new Error("Autoplay blocked")) : Promise.resolve(); },
    pause() { calls.pauses++; },
    load() { calls.loads++; },
    removeAttribute(name: string) { if (name === "src") this.src = ""; },
  };
  let changed: (() => void) | undefined;
  const motion = {
    matches: reduced,
    addEventListener(_event: string, listener: () => void) { changed = listener; },
    removeEventListener(_event: string, listener: () => void) { if (changed === listener) changed = undefined; },
  };
  let intersect: IntersectionObserverCallback;
  const controller = new WorkflowCoverController(video as unknown as HTMLVideoElement, "https://assets.example.com/preview.mp4", motion as unknown as MediaQueryList, callback => {
    intersect = callback;
    return { observe() {}, disconnect() { calls.disconnected = true; } };
  });
  return { controller, video, calls,
    visible(value: boolean) { intersect([{ target: video, isIntersecting: value }] as unknown as IntersectionObserverEntry[], {} as IntersectionObserver); },
    reduced(value: boolean) { motion.matches = value; changed?.(); },
  };
}

test("covers defer video bytes until visible, pause offscreen and return to the poster for reduced motion", () => {
  const demo = fixture();
  expect(demo.video.src).toBe("");
  expect(demo.video.muted).toBe(true);
  demo.visible(true);
  expect(demo.video.src).toBe("https://assets.example.com/preview.mp4");
  expect(demo.calls.plays).toBe(1);
  demo.visible(false);
  expect(demo.calls.pauses).toBe(1);
  demo.reduced(true);
  expect(demo.video.src).toBe("");
  expect(demo.calls.loads).toBe(1);
  demo.visible(true);
  expect(demo.calls.plays).toBe(1);
  demo.reduced(false);
  expect(demo.calls.plays).toBe(2);
  demo.controller.dispose();
  expect(demo.calls.disconnected).toBe(true);
  expect(demo.video.src).toBe("");
  demo.visible(true);
  expect(demo.calls.plays).toBe(2);
});

test("reduced-motion visitors keep the poster without fetching video, and refused autoplay is harmless", async () => {
  const still = fixture(true);
  still.visible(true);
  expect(still.video.src).toBe("");
  expect(still.calls.plays).toBe(0);
  still.controller.dispose();
  const refused = fixture(false, true);
  refused.visible(true);
  await Promise.resolve();
  expect(refused.calls.plays).toBe(1);
  refused.controller.dispose();
});
