import type { CanvasPreview } from "./graph";
import { VideoControls } from "./video-controls";

/** The widest first frame kept, in pixels: a card's frame is 328 canvas units wide. */
const STILL_WIDTH = 640;
/** How many videos load at once for their first frames. */
const LOADS = 2;
/** How long the pointer rests on a video's frame before it plays there, muted, in milliseconds. */
const HOVER_DELAY = 300;

type Box = Pick<DOMRectReadOnly, "left" | "top" | "width" | "height">;

/** Where a video stopped at `time` plays from next: there, or from the start once it has played through. */
export function resumeAt(time: number, duration: number): number | null {
  if (!Number.isFinite(time) || time <= 0) return null;
  return Number.isFinite(duration) && time >= duration - 0.25 ? null : time;
}

/** Where the player goes, in the host's pixels: over the frame as it is on screen now. */
export function playerBox(frame: Box, host: Box) {
  return { left: frame.left - host.left, top: frame.top - host.top, width: frame.width, height: frame.height };
}

/**
 * A video's first frame as a picture, or null when the video does not load or this page may not
 * read its pixels: a file on another host must allow this page (CORS), or it does not load.
 */
async function firstFrame(url: string, signal: AbortSignal): Promise<Blob | null> {
  const video = document.createElement("video");
  video.crossOrigin = "anonymous";
  video.muted = true;
  video.playsInline = true;
  video.preload = "auto";
  try {
    const loaded = await new Promise<boolean>((resolve) => {
      video.addEventListener("loadeddata", () => resolve(true), { once: true });
      video.addEventListener("error", () => resolve(false), { once: true });
      signal.addEventListener("abort", () => resolve(false), { once: true });
      video.src = url;
    });
    if (!loaded || !video.videoWidth || !video.videoHeight) return null;
    const scale = Math.min(1, STILL_WIDTH / video.videoWidth);
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(video.videoWidth * scale));
    canvas.height = Math.max(1, Math.round(video.videoHeight * scale));
    canvas.getContext("2d")?.drawImage(video, 0, 0, canvas.width, canvas.height);
    // WebP keeps transparency; a browser that cannot write it gives PNG.
    return await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/webp"));
  } catch {
    return null;
  } finally {
    video.removeAttribute("src");
    video.load();
  }
}

/**
 * Video previews on node cards. SVG has no video that every browser draws in place: Safari draws
 * an HTML video inside `foreignObject` outside the SVG's transforms and above the rest of it. So a
 * card shows its video's first frame as an ordinary SVG image, read once per file when the card
 * first comes into view, and one HTML player with the viewer's own controls (video-controls.ts),
 * laid over the card's frame, plays it: with sound from its play control, or muted and looping
 * while the pointer rests on the frame. The canvas stops the player when anything covers its card.
 */
export class CardVideos {
  /** First frames by digest: an object URL, or null when the file could not be read. */
  private readonly stills = new Map<string, Promise<string | null>>();
  private readonly ready = new Map<string, string | null>();
  /** Files the player could not play: their cards say so and offer no play control. */
  private readonly failed = new Set<string>();
  /** Where each file stopped short of its end: hovering or playing it again continues there. */
  private readonly positions = new Map<string, number>();
  /** What only the current video listens for; a stop ends it. */
  private session = new AbortController();
  private readonly waiting = new Map<Element, () => void>();
  private readonly observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      const load = entry.isIntersecting ? this.waiting.get(entry.target) : undefined;
      if (!load) continue;
      this.observer.unobserve(entry.target);
      this.waiting.delete(entry.target);
      load();
    }
  }, { rootMargin: "200px" });
  private readonly queue: (() => void)[] = [];
  private loading = 0;
  private readonly abort = new AbortController();
  private readonly controls: VideoControls;
  /** The player: the video and its controls, laid over the playing card's frame. */
  readonly player: HTMLDivElement;
  private readonly media: HTMLVideoElement;
  /** The video playing, and whether it plays only while hovered (muted, looping, until the pointer leaves). */
  private playing: { node: string; preview: CanvasPreview; card: SVGGElement; video: SVGGElement; hover: boolean } | null = null;
  private frame: number | null = null;
  private hoverTimer: number | null = null;
  // Playing on hover needs a pointer that hovers, and is motion a viewer may have asked to avoid.
  private readonly canHover = window.matchMedia("(hover: hover)");
  private readonly reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

  constructor(private readonly host: HTMLElement) {
    const signal = this.abort.signal;
    // Leaving a hovered video stops it. Pressing it keeps it playing, and a press on its picture
    // turns its sound on rather than pausing it.
    this.controls = new VideoControls(signal, () => this.stop(), (picture) => {
      if (!this.playing?.hover) return false;
      this.playing.hover = false;
      this.media.loop = false;
      if (picture) this.media.muted = false;
      this.player.focus({ preventScroll: true });
      return picture;
    });
    this.player = this.controls.element;
    this.media = this.controls.video;
    this.media.addEventListener("ended", () => this.stop(), { signal });
    this.media.addEventListener("error", () => {
      if (!this.playing || !this.media.getAttribute("src")) return;
      this.failed.add(this.playing.preview.digest);
      this.playing.video.classList.add("is-unavailable");
      this.stop();
    }, { signal });
    this.player.addEventListener("pointerleave", () => { if (this.playing?.hover) this.stop(); }, { signal });
  }

  /** The card whose video plays. */
  get card(): SVGGElement | null { return this.playing?.card ?? null; }

  /** Wires a drawn card's video: its first frame once the card is in view, and its play control. */
  attach(card: SVGGElement, node: string, preview: CanvasPreview, signal: AbortSignal) {
    const video = card.querySelector<SVGGElement>(".fx-canvas-video");
    const image = video?.querySelector("image");
    const play = video?.querySelector<SVGGElement>(".fx-canvas-play");
    if (!video || !image || !play) return;
    if (this.failed.has(preview.digest)) video.classList.add("is-unavailable");
    const show = (url: string | null) => { if (url && !signal.aborted) image.setAttribute("href", url); };
    if (this.ready.has(preview.digest)) show(this.ready.get(preview.digest)!);
    else {
      this.waiting.set(image, () => void this.still(preview).then(show));
      this.observer.observe(image);
      signal.addEventListener("abort", () => { this.observer.unobserve(image); this.waiting.delete(image); }, { once: true });
    }
    // A labelled control: it plays, and neither selects the card nor opens its deck.
    const start = (event: Event) => {
      event.preventDefault();
      event.stopPropagation();
      this.play({ node, preview, card, video });
    };
    play.addEventListener("click", start, { signal });
    play.addEventListener("keydown", (event) => { if (event.key === "Enter" || event.key === " ") start(event); }, { signal });
    // Resting on the frame plays it muted, unless a video played with sound is on.
    video.addEventListener("pointerenter", () => {
      this.cancelHover();
      if (!this.canHover.matches || this.reducedMotion.matches || this.failed.has(preview.digest) || (this.playing && !this.playing.hover)) return;
      this.hoverTimer = window.setTimeout(() => {
        this.hoverTimer = null;
        if (video.isConnected) this.play({ node, preview, card, video }, true);
      }, HOVER_DELAY);
    }, { signal });
    // The player appears under the pointer: moving onto it stays on the frame.
    video.addEventListener("pointerleave", (event) => {
      if (this.player.contains(event.relatedTarget as Node | null)) return;
      this.cancelHover();
      if (this.playing?.hover && this.playing.video === video) this.stop();
    }, { signal });
  }

  /** After a redraw the player stays with its step while the step's card shows the same file. */
  redrawn(cards: ReadonlyMap<string, SVGGElement>) {
    if (!this.playing) return;
    const card = cards.get(this.playing.node);
    const video = card?.querySelector<SVGGElement>(`.fx-canvas-video[data-digest="${this.playing.preview.digest}"]`);
    if (!card || !video) { this.stop(); return; }
    video.classList.add("is-playing");
    Object.assign(this.playing, { card, video });
  }

  stop() {
    this.cancelHover();
    const playing = this.playing;
    if (!playing) return;
    const focused = this.player.contains(document.activeElement);
    this.playing = null;
    this.session.abort();
    const at = resumeAt(this.media.currentTime, this.media.duration);
    if (at === null) this.positions.delete(playing.preview.digest);
    else this.positions.set(playing.preview.digest, at);
    if (this.frame !== null) cancelAnimationFrame(this.frame);
    this.frame = null;
    playing.video.classList.remove("is-playing");
    this.controls.reset();
    this.media.pause();
    this.player.hidden = true;
    this.media.removeAttribute("src");
    this.media.load();
    if (focused) playing.video.querySelector<SVGGElement>(".fx-canvas-play")?.focus({ preventScroll: true });
  }

  dispose() {
    this.stop();
    this.abort.abort();
    this.observer.disconnect();
    this.waiting.clear();
    for (const url of this.ready.values()) if (url) URL.revokeObjectURL(url);
    this.ready.clear();
  }

  private play(playing: Omit<NonNullable<CardVideos["playing"]>, "hover">, hover = false) {
    this.stop();
    this.playing = { ...playing, hover };
    playing.video.classList.add("is-playing");
    this.player.setAttribute("aria-label", playing.preview.label);
    this.media.muted = hover;
    this.media.loop = hover;
    this.session = new AbortController();
    // A video it stopped short of its end continues there. One starting over shows its first frame
    // until the video's own does; the card's backdrop shows around it.
    const { digest, url } = playing.preview;
    const at = this.positions.get(digest);
    const still = at === undefined ? this.ready.get(digest) : undefined;
    if (still) this.media.poster = still;
    else this.media.removeAttribute("poster");
    this.media.src = url;
    if (at !== undefined) {
      this.media.currentTime = at;
      // Set before the video has loaded, the time is where it starts; a browser that drops it seeks once it can.
      this.media.addEventListener("loadedmetadata", () => { if (this.media.currentTime < at - 0.5) this.media.currentTime = at; }, { once: true, signal: this.session.signal });
    }
    this.player.hidden = false;
    this.follow();
    // Hovering never takes focus.
    if (!hover) this.player.focus({ preventScroll: true });
    void this.media.play().catch(() => {});
  }
  private cancelHover() {
    if (this.hoverTimer !== null) window.clearTimeout(this.hoverTimer);
    this.hoverTimer = null;
  }

  /**
   * Keeps the player over its card's frame every frame, as the camera and the decks move it; full
   * screen, the browser places it.
   */
  private follow = () => {
    const frame = this.playing?.video.querySelector(".fx-canvas-image-border");
    if (!frame) return;
    if (!this.controls.fullscreened) {
      const box = playerBox(frame.getBoundingClientRect(), this.host.getBoundingClientRect());
      for (const side of ["left", "top", "width", "height"] as const) this.player.style[side] = `${box[side]}px`;
    }
    this.frame = requestAnimationFrame(this.follow);
  };

  /** A file's first frame, read once and at most `LOADS` at a time. */
  private still(preview: CanvasPreview): Promise<string | null> {
    let still = this.stills.get(preview.digest);
    if (still) return still;
    const signal = this.abort.signal;
    still = new Promise<void>((resolve) => { this.queue.push(resolve); this.next(); })
      .then(() => firstFrame(preview.url, signal))
      .then((blob) => {
        this.loading--;
        this.next();
        const url = blob && !signal.aborted ? URL.createObjectURL(blob) : null;
        this.ready.set(preview.digest, url);
        return url;
      });
    this.stills.set(preview.digest, still);
    return still;
  }
  private next() {
    while (this.loading < LOADS && this.queue.length) {
      this.loading++;
      this.queue.shift()!();
    }
  }
}
