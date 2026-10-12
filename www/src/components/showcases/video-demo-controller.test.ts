import { expect, test } from "bun:test";
import { VideoDemoPlayback } from "./video-demo-controller";

function fixture(reduced = false) {
  const calls = { plays: 0, pauses: 0, loads: 0, disconnected: false, attached: [] as boolean[] };
  const video = {
    src: "", preload: "none",
    play() { calls.plays++; return Promise.resolve(); },
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
  const player = new VideoDemoPlayback(video as unknown as HTMLVideoElement, "https://assets.example.com/final.mp4", attached => calls.attached.push(attached), motion as unknown as MediaQueryList, callback => {
    intersect = callback;
    return { observe() {}, disconnect() { calls.disconnected = true; } };
  });
  return { player, video, calls,
    visible(value: boolean) { intersect([{ target: video, isIntersecting: value }] as unknown as IntersectionObserverEntry[], {} as IntersectionObserver); },
    reduced(value: boolean) { motion.matches = value; changed?.(); },
  };
}

test("video examples load directly when visible, retain native playback and pause when hidden", () => {
  const demo = fixture();
  expect(demo.video.src).toBe("");
  demo.visible(true);
  expect(demo.video.src).toBe("https://assets.example.com/final.mp4");
  expect(demo.video.preload).toBe("metadata");
  expect(demo.calls.plays).toBe(0);
  demo.player.play();
  expect(demo.calls.plays).toBe(1);
  demo.visible(false);
  expect(demo.calls.pauses).toBe(1);
  demo.visible(true);
  expect(demo.calls.plays).toBe(1);
  demo.player.dispose();
  expect(demo.calls.disconnected).toBe(true);
  expect(demo.video.src).toBe("");
  demo.visible(true); demo.player.play();
  expect(demo.calls.plays).toBe(1);
});

test("reduced motion keeps the poster until explicit playback and responds to preference changes", () => {
  const demo = fixture(true);
  demo.visible(true);
  expect(demo.video.src).toBe("");
  expect(demo.calls.plays).toBe(0);
  demo.player.play();
  expect(demo.video.src).toBe("https://assets.example.com/final.mp4");
  expect(demo.calls.plays).toBe(1);
  demo.reduced(true);
  expect(demo.video.src).toBe("");
  expect(demo.calls.attached.at(-1)).toBe(false);
  demo.reduced(false);
  expect(demo.video.src).toBe("https://assets.example.com/final.mp4");
  expect(demo.calls.plays).toBe(1);
  demo.player.dispose();
});
