import { expect, test } from "bun:test";
import { mediaFailure, videoFailureLabel, videoFailureText, videoInteraction, waitForVideoFrame, type VideoFailure } from "../src/video";

/** Only the native media events and assigned URL matter while waiting for the first frame. */
class Video extends EventTarget {
  src = "";
  error: Pick<MediaError, "code" | "message"> | null = null;
  readonly listening = new Set<string>();

  override addEventListener(type: string, listener: EventListenerOrEventListenerObject | null, options?: AddEventListenerOptions | boolean) {
    this.listening.add(type);
    super.addEventListener(type, listener, options);
  }
  override removeEventListener(type: string, listener: EventListenerOrEventListenerObject | null, options?: EventListenerOptions | boolean) {
    this.listening.delete(type);
    super.removeEventListener(type, listener, options);
  }
  get element() { return this as unknown as HTMLVideoElement; }
}

test("native first-frame data and errors settle the probe and remove media listeners", async () => {
  for (const event of ["loadeddata", "error"]) {
    const video = new Video(), abort = new AbortController();
    const result = waitForVideoFrame(video.element, "/artifact.mkv", abort.signal);
    expect(video.src).toBe("/artifact.mkv");
    video.dispatchEvent(new Event(event));
    expect(await result).toEqual(event === "loadeddata" ? { ok: true } : { ok: false, failure: { category: "source" } });
    expect(video.listening.size).toBe(0);
    abort.abort();
  }
});

test("a silent media load reaches its deadline and leaves the next probe free to load", async () => {
  const video = new Video();
  expect(await waitForVideoFrame(video.element, "/stalled.mkv", new AbortController().signal, 1)).toEqual({ ok: false, failure: { category: "timeout" } });
  expect(video.listening.size).toBe(0);
  const next = waitForVideoFrame(video.element, "/next.webm", new AbortController().signal);
  video.dispatchEvent(new Event("loadeddata"));
  expect(await next).toEqual({ ok: true });
  expect(video.listening.size).toBe(0);
});

test("aborting a pending probe releases it, and an already aborted probe starts no request", async () => {
  const video = new Video(), abort = new AbortController();
  const result = waitForVideoFrame(video.element, "/loading.mov", abort.signal);
  abort.abort();
  expect(await result).toEqual({ ok: false, failure: { category: "aborted" } });
  expect(video.listening.size).toBe(0);
  expect(await waitForVideoFrame(video.element, "/never-requested.mov", abort.signal)).toEqual({ ok: false, failure: { category: "aborted" } });
  expect(video.src).toBe("/loading.mov");
});

test("native media reasons preserve uncertainty about source failures", () => {
  expect([1, 2, 3, 4].map(code => mediaFailure({ code, message: "" }).category)).toEqual(["aborted", "network", "decode", "source"]);
  const source = videoFailureText(mediaFailure({ code: 4, message: "" }));
  expect(source.reason).toBe("Browser could not load this video");
  expect(source.details).toContain("Browser media error 4");
  expect(source.details).not.toContain("codec");
});

test("a probe captures native diagnostics before resetting media, with bounded plain text", async () => {
  const video = new Video();
  const result = waitForVideoFrame(video.element, "/native.mkv", new AbortController().signal);
  video.error = { code: 3, message: `  Frame\n\t decode\u0000failed: <detail> ${"x".repeat(300)}` };
  video.dispatchEvent(new Event("error"));
  video.error = null;
  const loaded = await result;
  expect(loaded.ok).toBe(false);
  if (loaded.ok) throw new Error("The failing media must retain its diagnostic");
  expect(loaded.failure.category).toBe("decode");
  expect(loaded.failure.code).toBe(3);
  expect(loaded.failure.native_message).toStartWith("Frame decode failed: <detail> ");
  expect(loaded.failure.native_message).toHaveLength(240);
  expect(videoFailureText(loaded.failure).details).toStartWith("Browser could not decode the video. Browser media error 3. Frame decode failed");
});

test("hover playback requires a successfully captured first frame", () => {
  expect(videoInteraction(undefined)).toEqual({ control: "play", hover: false });
  expect(videoInteraction("blob:ready")).toEqual({ control: "play", hover: true });
});

test("native load failures and playback failures expose no playback action", () => {
  for (const category of ["aborted", "network", "decode", "source", "timeout", "omitted", "unavailable"] satisfies VideoFailure["category"][]) {
    expect(videoInteraction(null, { category })).toEqual({ control: "unavailable", hover: false });
    // A stale still cannot bypass failure when a scheduled hover checks again.
    expect(videoInteraction("blob:earlier", { category })).toEqual({ control: "unavailable", hover: false });
  }
  expect(videoInteraction("blob:ready", undefined, { category: "decode" })).toEqual({ control: "unavailable", hover: false });
});

test("capture-only failures offer an explicit retry and never autoplay", () => {
  expect(videoInteraction(null, { category: "capture" })).toEqual({ control: "try", hover: false });
  expect(videoInteraction("blob:earlier", { category: "capture" })).toEqual({ control: "try", hover: false });
  expect(videoInteraction(null, { category: "capture" }, { category: "source" })).toEqual({ control: "unavailable", hover: false });
});

test("an available poster remains a preview even when browser playback or capture fails", () => {
  for (const category of ["source", "decode", "network", "omitted", "unavailable"] satisfies VideoFailure["category"][]) {
    expect(videoFailureLabel(true, { category })).toBe("Browser playback unavailable");
    expect(videoFailureLabel(false, { category })).toBe("Browser preview unavailable");
  }
  expect(videoFailureLabel(true, { category: "capture" })).toBe("Poster preview available");
  expect(videoFailureLabel(false, { category: "capture" })).toBe("Browser preview unavailable");
});
