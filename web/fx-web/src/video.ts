import type { CanvasPreview } from "./graph";
import { VideoControls } from "./video-controls";

/** The widest first frame kept, in pixels: a card's frame is 328 canvas units wide. */
const STILL_WIDTH = 640;
/** How many videos load at once for their first frames. */
const LOADS = 2;
/** How long the pointer rests on a video's frame before it plays there, muted, in milliseconds. */
const HOVER_DELAY = 300;
/** How long the player fades in and out over the card's first frame, in milliseconds (canvas.css). */
const FADE = 150;
/** How long a stopped video stays loaded, so playing it again continues at once, in milliseconds. */
const KEEP_LOADED = 30_000;

/** Where a video stopped at `time` plays from next: there, or from the start once it has played through. */
export function resumeAt(time: number, duration: number): number | null {
  if (!Number.isFinite(time) || time <= 0) return null;
  return Number.isFinite(duration) && time >= duration - 0.25 ? null : time;
}

/**
 * A video's first frame drawn on a canvas, or null when the video does not load or this page may
 * not read its pixels: a file on another host must allow this page (CORS), or it does not load.
 * The video lets go of its file before the caller encodes the frame.
 */
async function drawFirstFrame(url: string, signal: AbortSignal): Promise<HTMLCanvasElement | null> {
  if (signal.aborted) return null;
  const video = document.createElement("video");
  video.crossOrigin = "anonymous";
  video.muted = true;
  video.playsInline = true;
  video.preload = "auto";
  let settle = (_loaded: boolean) => {};
  const abort = () => settle(false);
  signal.addEventListener("abort", abort, { once: true });
  try {
    const loaded = await new Promise<boolean>((resolve) => {
      settle = resolve;
      video.addEventListener("loadeddata", () => resolve(true), { once: true });
      video.addEventListener("error", () => resolve(false), { once: true });
      video.src = url;
    });
    if (!loaded || !video.videoWidth || !video.videoHeight) return null;
    const scale = Math.min(1, STILL_WIDTH / video.videoWidth);
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(video.videoWidth * scale));
    canvas.height = Math.max(1, Math.round(video.videoHeight * scale));
    canvas.getContext("2d")?.drawImage(video, 0, 0, canvas.width, canvas.height);
    return canvas;
  } catch {
    return null;
  } finally {
    signal.removeEventListener("abort", abort);
    video.removeAttribute("src");
    video.load();
  }
}

/** A video's first frame as a picture, or null (drawFirstFrame). */
async function firstFrame(url: string, signal: AbortSignal): Promise<Blob | null> {
  const canvas = await drawFirstFrame(url, signal);
  if (!canvas) return null;
  try {
    // WebP keeps transparency; a browser that cannot write it gives PNG.
    return await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/webp"));
  } catch {
    return null;
  } finally {
    canvas.width = canvas.height = 0;
  }
}

/** The playing video: its step, its file, its card's video group and frame, and whether it only plays while hovered. */
interface Playing { node: string; preview: CanvasPreview; video: SVGGElement; frame: Element; hover: boolean }

/**
 * Video previews on node cards. SVG has no video that every browser draws in place: Safari draws
 * an HTML video inside `foreignObject` outside the SVG's transforms and above the rest of it. So a
 * card shows its video's first frame as an ordinary SVG image, read once per file when the card
 * first comes into view, and one HTML player with the viewer's own controls (video-controls.ts),
 * laid over the card's frame, plays it: with sound from its play control, or muted and looping
 * while the pointer rests on the frame. It never plays, or goes on playing, over a covered card:
 * the canvas says what covers one.
 */
export class CardVideos {
  /** First frames by digest: an object URL, null when the file could not be read, undefined while reading. */
  private readonly stills = new Map<string, string | null | undefined>();
  /** Files the player could not play: their cards say so and offer no play control. */
  private readonly failed = new Set<string>();
  /** Where each file stopped short of its end: hovering or playing it again continues there. */
  private readonly positions = new Map<string, number>();
  /** First frames waiting for their card to come into view. */
  private readonly waiting = new Map<Element, CanvasPreview>();
  private readonly observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      const preview = entry.isIntersecting ? this.waiting.get(entry.target) : undefined;
      if (!preview) continue;
      this.observer.unobserve(entry.target);
      this.waiting.delete(entry.target);
      this.read(preview);
    }
  }, { rootMargin: "200px" });
  private readonly queue: { digest: string; url: string }[] = [];
  private loading = 0;
  private readonly abort = new AbortController();
  private readonly controls: VideoControls;
  /** The player: the video and its controls, laid over the playing card's frame. */
  readonly player: HTMLDivElement;
  private readonly media: HTMLVideoElement;
  private playing: Playing | null = null;
  /** The player's last placement, written only when it changes. */
  private box = "";
  // Timer and frame handles; clearing 0 or a spent one does nothing.
  private following = 0;
  private hoverTimer = 0;
  private hideTimer = 0;
  private unloadTimer = 0;

  constructor(private readonly host: HTMLElement, private readonly hoverPlays: () => boolean, private readonly covered: (element: Element) => boolean) {
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
      this.unload();
    }, { signal });
    for (const type of ["loadeddata", "seeked", "playing"] as const) this.media.addEventListener(type, this.settle, { signal });
    // A time set before the video loaded is where it starts; a browser that drops it seeks once it can.
    this.media.addEventListener("loadedmetadata", () => {
      const at = this.playing && this.positions.get(this.playing.preview.digest);
      if (at && this.media.currentTime < at - 0.5) this.media.currentTime = at;
    }, { signal });
    this.player.addEventListener("pointerleave", () => { if (this.playing?.hover) this.stop(); }, { signal });
  }

  /** Wires a drawn card's video: its first frame once the card is in view, and its play control. */
  attach(card: SVGGElement, node: string, preview: CanvasPreview, signal: AbortSignal) {
    const video = card.querySelector<SVGGElement>(".fx-canvas-video");
    const image = video?.querySelector("image");
    const frame = video?.querySelector(".fx-canvas-image-border");
    const play = video?.querySelector<SVGGElement>(".fx-canvas-play");
    if (!video || !image || !frame || !play) return;
    if (this.failed.has(preview.digest)) video.classList.add("is-unavailable");
    const still = this.stills.get(preview.digest);
    if (still) image.setAttribute("href", still);
    // A first frame already being read reaches this card when it is ready (read).
    else if (!this.stills.has(preview.digest)) {
      this.waiting.set(image, preview);
      this.observer.observe(image);
      signal.addEventListener("abort", () => { this.observer.unobserve(image); this.waiting.delete(image); }, { once: true });
    }
    // A labelled control: it plays, and neither selects the card nor opens its deck.
    const start = (event: Event) => {
      event.preventDefault();
      event.stopPropagation();
      this.play({ node, preview, video, frame });
    };
    play.addEventListener("click", start, { signal });
    play.addEventListener("keydown", (event) => { if (event.key === "Enter" || event.key === " ") start(event); }, { signal });
    // Resting on the frame plays it muted, unless a video played with sound is on. A press waits
    // for its release, so the player never appears under a click.
    const rest = () => {
      window.clearTimeout(this.hoverTimer);
      if (!this.hoverPlays() || this.failed.has(preview.digest)) return;
      this.hoverTimer = window.setTimeout(() => {
        if (video.isConnected && !(this.playing && (!this.playing.hover || this.playing.video === video))) this.play({ node, preview, video, frame }, true);
      }, HOVER_DELAY);
    };
    video.addEventListener("pointerenter", rest, { signal });
    video.addEventListener("pointerdown", () => window.clearTimeout(this.hoverTimer), { signal });
    video.addEventListener("pointerup", rest, { signal });
    // The player appears under the pointer: moving onto it stays on the frame.
    video.addEventListener("pointerleave", (event) => {
      if (this.player.contains(event.relatedTarget as Node | null)) return;
      window.clearTimeout(this.hoverTimer);
      if (this.playing?.hover && this.playing.video === video) this.stop();
    }, { signal });
  }

  /** After a redraw the player stays with its step while the step's card shows the same file. */
  redrawn(cards: ReadonlyMap<string, SVGGElement>) {
    if (!this.playing) return;
    const video = cards.get(this.playing.node)?.querySelector<SVGGElement>(`.fx-canvas-video[data-digest="${this.playing.preview.digest}"]`);
    const frame = video?.querySelector(".fx-canvas-image-border");
    if (!video || !frame) { this.stop(); return; }
    video.classList.toggle("is-shown", this.player.classList.contains("is-shown"));
    Object.assign(this.playing, { video, frame });
  }

  /** Stops the video if something now covers its card. */
  recheck() {
    if (this.playing && this.covered(this.playing.video)) this.stop();
  }

  stop() {
    window.clearTimeout(this.hoverTimer);
    const playing = this.playing;
    if (!playing) return;
    const focused = this.player.contains(document.activeElement);
    this.playing = null;
    const at = resumeAt(this.media.currentTime, this.media.duration);
    if (at === null) this.positions.delete(playing.preview.digest);
    else this.positions.set(playing.preview.digest, at);
    cancelAnimationFrame(this.following);
    this.box = "";
    this.show(playing.video, false);
    this.controls.reset();
    // Paused, not unloaded: playing it again shows this frame at once and goes on from it. It fades
    // out over the card's first frame, which shows again at once.
    this.media.pause();
    this.hideTimer = window.setTimeout(() => { this.player.hidden = true; }, FADE);
    this.unloadTimer = window.setTimeout(() => this.unload(), KEEP_LOADED);
    if (focused) playing.video.querySelector<SVGGElement>(".fx-canvas-play")?.focus({ preventScroll: true });
  }

  dispose() {
    this.stop();
    window.clearTimeout(this.hideTimer);
    this.unload();
    this.abort.abort();
    this.observer.disconnect();
    this.waiting.clear();
    for (const still of this.stills.values()) if (still) URL.revokeObjectURL(still);
    this.stills.clear();
  }

  private play(playing: Omit<Playing, "hover">, hover = false) {
    if (this.covered(playing.video)) return;
    this.stop();
    window.clearTimeout(this.hideTimer);
    window.clearTimeout(this.unloadTimer);
    this.playing = { ...playing, hover };
    this.player.setAttribute("aria-label", playing.preview.label);
    this.media.muted = hover;
    this.media.loop = hover;
    // A video stopped short of its end continues there: at once while the player still holds it,
    // else from where it starts loading.
    const { digest, url } = playing.preview;
    const at = this.positions.get(digest) ?? 0;
    if (this.media.getAttribute("src") !== url) {
      this.media.src = url;
      if (at) this.media.currentTime = at;
    } else if (Math.abs(this.media.currentTime - at) > 0.05) this.media.currentTime = at;
    this.settle();
    this.player.hidden = false;
    this.follow();
    // Hovering never takes focus.
    if (!hover) this.player.focus({ preventScroll: true });
    void this.media.play().catch(() => {});
  }

  /**
   * The card keeps its first frame, and the player stays clear, until the video shows its own frame
   * there: nothing blanks while it loads, and its controls never show a time it leaves.
   */
  private settle = () => {
    if (this.playing && this.media.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA && !this.media.seeking) this.show(this.playing.video, true);
  };
  /** Whether the video shows its frame over the card, which then hides its first frame and play control. */
  private show(video: SVGGElement, shown: boolean) {
    video.classList.toggle("is-shown", shown);
    this.player.classList.toggle("is-shown", shown);
  }
  /** Lets go of a stopped video's file and decoder. */
  private unload() {
    window.clearTimeout(this.unloadTimer);
    if (this.playing || !this.media.hasAttribute("src")) return;
    this.media.removeAttribute("src");
    this.media.load();
  }

  /**
   * Keeps the player over its card's frame every frame, as the camera and the decks move it; full
   * screen, the browser places it.
   */
  private follow = () => {
    if (!this.playing) return;
    if (!this.controls.fullscreened) {
      const frame = this.playing.frame.getBoundingClientRect(), host = this.host.getBoundingClientRect();
      const box = [frame.left - host.left, frame.top - host.top, frame.width, frame.height];
      const key = box.join();
      if (key !== this.box) {
        this.box = key;
        (["left", "top", "width", "height"] as const).forEach((side, at) => { this.player.style[side] = `${box[at]}px`; });
      }
    }
    this.following = requestAnimationFrame(this.follow);
  };

  /** Asks for a file's first frame once; at most `LOADS` read at a time. */
  private read({ digest, url }: CanvasPreview) {
    if (this.stills.has(digest)) return;
    this.stills.set(digest, undefined);
    this.queue.push({ digest, url });
    this.next();
  }
  private next() {
    while (this.loading < LOADS && this.queue.length) {
      const { digest, url } = this.queue.shift()!;
      this.loading++;
      void firstFrame(url, this.abort.signal).then((blob) => {
        this.loading--;
        this.next();
        if (this.abort.signal.aborted) return;
        const still = blob ? URL.createObjectURL(blob) : null;
        this.stills.set(digest, still);
        // Whichever cards show the file now; a card drawn since the read began among them.
        if (still) for (const image of this.host.querySelectorAll(`.fx-canvas-video[data-digest="${digest}"] image`)) image.setAttribute("href", still);
      });
    }
  }
}
