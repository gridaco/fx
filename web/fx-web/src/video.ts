import type { CanvasPreview } from "./graph";

/** The widest first frame kept, in pixels: a card's frame is 328 canvas units wide. */
const STILL_WIDTH = 640;
/** How many videos load at once for their first frames. */
const LOADS = 2;

type Box = Pick<DOMRectReadOnly, "left" | "top" | "width" | "height">;

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
 * first comes into view, and one HTML player, laid over the card's frame, plays it on request.
 * The canvas stops the player when anything covers its card.
 */
export class CardVideos {
  /** First frames by digest: an object URL, or null when the file could not be read. */
  private readonly stills = new Map<string, Promise<string | null>>();
  private readonly ready = new Map<string, string | null>();
  /** Files the player could not play: their cards say so and offer no play control. */
  private readonly failed = new Set<string>();
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
  readonly player = document.createElement("video");
  private playing: { node: string; preview: CanvasPreview; card: SVGGElement; video: SVGGElement } | null = null;
  private frame: number | null = null;

  constructor(private readonly host: HTMLElement) {
    const signal = this.abort.signal;
    this.player.className = "fx-canvas-player";
    this.player.controls = true;
    this.player.playsInline = true;
    this.player.hidden = true;
    this.player.addEventListener("ended", () => this.stop(), { signal });
    this.player.addEventListener("error", () => {
      if (!this.playing || !this.player.getAttribute("src")) return;
      this.failed.add(this.playing.preview.digest);
      this.playing.video.classList.add("is-unavailable");
      this.stop();
    }, { signal });
    this.player.addEventListener("keydown", (event) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      event.stopPropagation();
      this.stop();
    }, { signal });
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
    const playing = this.playing;
    if (!playing) return;
    const focused = document.activeElement === this.player;
    this.playing = null;
    if (this.frame !== null) cancelAnimationFrame(this.frame);
    this.frame = null;
    playing.video.classList.remove("is-playing");
    this.player.pause();
    this.player.hidden = true;
    this.player.removeAttribute("src");
    this.player.load();
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

  private play(playing: NonNullable<CardVideos["playing"]>) {
    this.stop();
    this.playing = playing;
    playing.video.classList.add("is-playing");
    this.player.setAttribute("aria-label", playing.preview.label);
    this.player.src = playing.preview.url;
    this.player.hidden = false;
    this.follow();
    this.player.focus({ preventScroll: true });
    void this.player.play().catch(() => {});
  }

  /** Keeps the player over its card's frame every frame, as the camera and the decks move it. */
  private follow = () => {
    const frame = this.playing?.video.querySelector(".fx-canvas-image-border");
    if (!frame) return;
    const box = playerBox(frame.getBoundingClientRect(), this.host.getBoundingClientRect());
    for (const side of ["left", "top", "width", "height"] as const) this.player.style[side] = `${box[side]}px`;
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
