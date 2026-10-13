import { createElement, Maximize, Minimize, Volume2, VolumeX, type IconNode } from "lucide";
import { mediaGlyph } from "./card";

/** How long the pointer rests before a playing video's controls fade, in milliseconds. */
const IDLE_DELAY = 2000;
/** How far the arrow keys seek, in seconds. */
const SEEK_STEP = 5;

/** A video time as players show it: "0:07", "12:30", "1:02:03". */
export function videoTime(seconds: number) {
  const whole = Number.isFinite(seconds) ? Math.max(0, Math.floor(seconds)) : 0;
  const [hours, minutes, rest] = [Math.floor(whole / 3600), Math.floor(whole / 60) % 60, whole % 60];
  const two = (value: number) => String(value).padStart(2, "0");
  return hours ? `${hours}:${two(minutes)}:${two(rest)}` : `${minutes}:${two(rest)}`;
}

/** Where a seek by `by` seconds lands, within the video. */
export function seekTarget(at: number, by: number, duration: number) {
  const end = Number.isFinite(duration) ? duration : at + Math.max(0, by);
  return Math.min(end, Math.max(0, at + by));
}

/** What a key does to a focused player; keys on its own buttons and slider stay theirs. */
export function playerKey(key: string, target: "button" | "slider" | "other"): "toggle" | "back" | "forward" | "mute" | "fullscreen" | "close" | null {
  if (key === "Escape") return "close";
  if ((key === " " || key === "Enter") && target === "button") return null;
  if ((key === "ArrowLeft" || key === "ArrowRight") && target === "slider") return null;
  switch (key.toLowerCase()) {
    case " ": case "k": return "toggle";
    case "arrowleft": return "back";
    case "arrowright": return "forward";
    case "m": return "mute";
    case "f": return "fullscreen";
    default: return null;
  }
}

/**
 * A video with the viewer's own controls: play, a seek slider, time, mute and full screen, sized in
 * screen pixels and fewer when the player is small on screen, so they stay usable at any zoom of
 * the canvas under it. While it plays they fade when the pointer rests; a press anywhere shows them.
 * `press` sees every press first: it returns true to take a press on the picture for itself.
 */
export class VideoControls {
  readonly element = document.createElement("div");
  readonly video = document.createElement("video");
  private readonly bar = document.createElement("div");
  private readonly playButton = this.button("fx-video-play");
  private readonly seek = document.createElement("input");
  private readonly time = document.createElement("span");
  private readonly muteButton = this.button("fx-video-mute");
  private readonly fullscreenButton = this.button("fx-video-fullscreen");
  private idle: number | null = null;
  private scrubbing = false;

  constructor(signal: AbortSignal, close: () => void, press: (picture: boolean) => boolean) {
    const { element, video, bar, seek } = this;
    element.className = "fx-canvas-player";
    element.tabIndex = -1;
    element.setAttribute("role", "group");
    element.hidden = true;
    video.playsInline = true;
    bar.className = "fx-video-controls";
    seek.type = "range";
    seek.className = "fx-video-seek";
    seek.min = "0";
    seek.step = "any";
    seek.setAttribute("aria-label", "Seek");
    this.time.className = "fx-video-time";
    this.fullscreenButton.hidden = !document.fullscreenEnabled && !("webkitEnterFullscreen" in video);
    bar.append(this.playButton, seek, this.time, this.muteButton, this.fullscreenButton);
    element.append(video, bar);

    const on = <K extends keyof HTMLVideoElementEventMap>(type: K, listener: () => void) => video.addEventListener(type, listener, { signal });
    for (const type of ["play", "pause", "timeupdate", "durationchange", "progress", "volumechange", "emptied"] as const) on(type, () => this.render());
    on("play", () => this.wake());
    on("pause", () => this.wake());
    document.addEventListener("fullscreenchange", () => this.render(), { signal });

    this.playButton.addEventListener("click", () => this.toggle(), { signal });
    this.muteButton.addEventListener("click", () => { video.muted = !video.muted; }, { signal });
    this.fullscreenButton.addEventListener("click", () => this.fullscreen(), { signal });
    seek.addEventListener("input", () => { video.currentTime = Number(seek.value); this.render(); }, { signal });
    seek.addEventListener("pointerdown", () => { this.scrubbing = true; }, { signal });
    for (const type of ["pointerup", "pointercancel", "change"] as const) seek.addEventListener(type, () => { this.scrubbing = false; }, { signal });

    // The picture plays and pauses, unless `press` took the press.
    let taken = false;
    element.addEventListener("pointerdown", (event) => { taken = press(event.target === video); this.wake(); }, { signal });
    video.addEventListener("click", () => { if (taken) taken = false; else this.toggle(); }, { signal });
    element.addEventListener("pointermove", () => this.wake(), { signal });
    element.addEventListener("pointerleave", () => this.rest(), { signal });
    element.addEventListener("focusin", () => this.wake(), { signal });
    element.addEventListener("keydown", (event) => {
      const target = event.target === seek ? "slider" : (event.target as Element).closest("button") ? "button" : "other";
      const action = playerKey(event.key, target);
      if (!action || event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
      event.preventDefault();
      event.stopPropagation();
      this.wake();
      if (action === "close") close();
      else if (action === "toggle") this.toggle();
      else if (action === "mute") video.muted = !video.muted;
      else if (action === "fullscreen") this.fullscreen();
      else video.currentTime = seekTarget(video.currentTime, action === "back" ? -SEEK_STEP : SEEK_STEP, video.duration);
    }, { signal });
    signal.addEventListener("abort", () => this.clearIdle(), { once: true });
    this.render();
  }

  /** Whether the player fills the screen: the canvas then leaves its size to the browser. */
  get fullscreened() { return document.fullscreenElement === this.element; }

  /** Before another video: controls shown, nothing pending. */
  reset() {
    this.clearIdle();
    this.scrubbing = false;
    this.element.classList.remove("is-idle");
    if (this.fullscreened) void document.exitFullscreen().catch(() => {});
  }

  private toggle() {
    if (this.video.paused) void this.video.play().catch(() => {});
    else this.video.pause();
  }
  private fullscreen() {
    if (this.fullscreened) void document.exitFullscreen().catch(() => {});
    else if (document.fullscreenEnabled) void this.element.requestFullscreen().catch(() => {});
    // iOS shows a video full screen only in its own player.
    else (this.video as HTMLVideoElement & { webkitEnterFullscreen?: () => void }).webkitEnterFullscreen?.();
  }

  /** Shows the controls; while the video plays they fade again once the pointer rests. */
  private wake() {
    this.clearIdle();
    this.element.classList.remove("is-idle");
    if (!this.video.paused) this.idle = window.setTimeout(() => this.rest(), IDLE_DELAY);
  }
  private rest() {
    this.clearIdle();
    if (!this.video.paused && !this.bar.contains(document.activeElement)) this.element.classList.add("is-idle");
  }
  private clearIdle() {
    if (this.idle !== null) window.clearTimeout(this.idle);
    this.idle = null;
  }

  private render() {
    const { video, seek } = this;
    const duration = Number.isFinite(video.duration) ? video.duration : 0;
    const playing = !video.paused;
    this.icon(this.playButton, () => mediaGlyph(playing ? "pause" : "play", { width: 15, height: 15 }), playing ? "Pause" : "Play");
    this.icon(this.muteButton, lucide(video.muted ? VolumeX : Volume2), video.muted ? "Unmute" : "Mute");
    this.icon(this.fullscreenButton, lucide(this.fullscreened ? Minimize : Maximize), this.fullscreened ? "Exit full screen" : "Full screen");
    seek.max = String(duration);
    seek.disabled = duration === 0;
    if (!this.scrubbing) seek.value = String(video.currentTime);
    const share = (at: number) => `${duration ? Math.min(100, (at / duration) * 100) : 0}%`;
    const buffered = video.buffered.length ? video.buffered.end(video.buffered.length - 1) : 0;
    seek.style.setProperty("--fx-played", share(video.currentTime));
    seek.style.setProperty("--fx-buffered", share(buffered));
    const [at, length] = [videoTime(video.currentTime), videoTime(duration)];
    const text = `${at} / ${length}`;
    seek.setAttribute("aria-valuetext", `${at} of ${length}`);
    if (this.time.textContent !== text) this.time.textContent = text;
  }

  private button(className: string) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = className;
    return button;
  }
  /** Swaps a button's icon and name only when they change. */
  private icon(button: HTMLButtonElement, icon: () => SVGElement, label: string) {
    if (button.getAttribute("aria-label") === label) return;
    button.setAttribute("aria-label", label);
    button.title = label;
    button.replaceChildren(icon());
  }
}

const lucide = (icon: IconNode) => () => createElement(icon, { width: 15, height: 15, "stroke-width": 2, "aria-hidden": "true" });
