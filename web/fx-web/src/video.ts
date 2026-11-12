import type { CanvasVideoPreview } from "./graph";
import { VideoControls } from "./video-controls";

/** The widest first frame kept, in pixels: a card's frame is 328 canvas units wide. */
const STILL_WIDTH = 640;
/** How many videos load at once for their first frames. */
const LOADS = 2;
/** A stalled file must release its first-frame loading slot. */
const FRAME_TIMEOUT = 10_000;
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

export interface VideoFailure {
  category: "aborted" | "network" | "decode" | "source" | "capture" | "timeout" | "omitted" | "unavailable";
  code?: number;
  native_message?: string;
}
type VideoLoadResult = { ok: true } | { ok: false; failure: VideoFailure };
type VideoFrameResult<T> = { ok: true; frame: T } | { ok: false; failure: VideoFailure };

const reasons: Record<VideoFailure["category"], string> = {
  aborted: "Loading was interrupted",
  network: "Network error while loading video",
  decode: "Browser could not decode the video",
  source: "Browser could not load this video",
  capture: "Could not capture a video frame",
  timeout: "Loading timed out",
  omitted: "Video is not included in this recording",
  unavailable: "Source video is unavailable",
};

function failure(category: VideoFailure["category"], message?: string, code?: number): VideoFailure {
  // Browser diagnostics stay in this page's memory and DOM, never in a run or a log.
  const native_message = message?.replace(/[\u0000-\u001f\u007f]/g, " ").replace(/\s+/g, " ").trim().slice(0, 240);
  return { category, ...(code !== undefined ? { code } : {}), ...(native_message ? { native_message } : {}) };
}

/** Native code 4 does not distinguish a missing source, unsuitable response, or unsupported format. */
export function mediaFailure(error: Pick<MediaError, "code" | "message"> | null): VideoFailure {
  const category = error?.code === 1 ? "aborted" : error?.code === 2 ? "network" : error?.code === 3 ? "decode" : "source";
  return failure(category, error?.message, error?.code);
}

function captureFailure(error?: unknown): VideoFailure {
  return failure("capture", error instanceof Error ? error.message : undefined);
}

export function videoFailureText(error: VideoFailure): { reason: string; details: string } {
  const reason = reasons[error.category];
  return { reason, details: [reason, ...(error.code !== undefined ? [`Browser media error ${error.code}`] : []), ...(error.native_message ? [error.native_message] : [])].join(". ") };
}

export function videoFailureLabel(posterLoaded: boolean, error: VideoFailure): string {
  return posterLoaded ? error.category === "capture" ? "Poster preview available" : "Browser playback unavailable" : "Browser preview unavailable";
}

/** Only a ready preview autoplays; a capture failure permits a clearly labelled manual attempt. */
export function videoInteraction(still: string | null | undefined, previewFailure?: VideoFailure, playbackFailure?: VideoFailure): { control: "play" | "try" | "unavailable"; hover: boolean } {
  const control = playbackFailure || (previewFailure && previewFailure.category !== "capture") ? "unavailable"
    : previewFailure ? "try" : "play";
  return { control, hover: Boolean(still) && !previewFailure && !playbackFailure };
}

/** Wait for native decoding, with a bound even when the browser emits neither data nor an error. */
export function waitForVideoFrame(video: HTMLVideoElement, url: string, signal: AbortSignal, timeout = FRAME_TIMEOUT): Promise<VideoLoadResult> {
  if (signal.aborted) return Promise.resolve({ ok: false, failure: failure("aborted") });
  return new Promise((resolve) => {
    const finish = (result: VideoLoadResult) => {
      clearTimeout(timer);
      video.removeEventListener("loadeddata", loadedData);
      video.removeEventListener("error", failed);
      signal.removeEventListener("abort", aborted);
      resolve(result);
    };
    const loadedData = () => finish({ ok: true });
    const failed = () => finish({ ok: false, failure: mediaFailure(video.error) });
    const aborted = () => finish({ ok: false, failure: failure("aborted") });
    const timer = setTimeout(() => finish({ ok: false, failure: failure("timeout") }), timeout);
    video.addEventListener("loadeddata", loadedData);
    video.addEventListener("error", failed);
    signal.addEventListener("abort", aborted, { once: true });
    try { video.src = url; }
    catch (error) { finish({ ok: false, failure: failure("source", error instanceof Error ? error.message : undefined) }); }
  });
}

/**
 * A video's first frame drawn on a canvas, or the reason loading or reading its pixels failed.
 * A file on another host must allow this page (CORS), or it does not load for pixel capture.
 * The video lets go of its file before the caller encodes the frame.
 */
async function drawFirstFrame(url: string, signal: AbortSignal): Promise<VideoFrameResult<HTMLCanvasElement>> {
  if (signal.aborted) return { ok: false, failure: failure("aborted") };
  const video = document.createElement("video");
  video.crossOrigin = "anonymous";
  video.muted = true;
  video.playsInline = true;
  video.preload = "auto";
  try {
    const loaded = await waitForVideoFrame(video, url, signal);
    if (!loaded.ok) return loaded;
    if (!video.videoWidth || !video.videoHeight) return { ok: false, failure: failure("decode", "The browser did not expose a video frame.") };
    const scale = Math.min(1, STILL_WIDTH / video.videoWidth);
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(video.videoWidth * scale));
    canvas.height = Math.max(1, Math.round(video.videoHeight * scale));
    const context = canvas.getContext("2d");
    if (!context) return { ok: false, failure: captureFailure() };
    context.drawImage(video, 0, 0, canvas.width, canvas.height);
    return { ok: true, frame: canvas };
  } catch (error) {
    return { ok: false, failure: captureFailure(error) };
  } finally {
    video.removeAttribute("src");
    video.load();
  }
}

/** A video's first frame as a picture, retaining browser-local failure details. */
async function firstFrame(url: string, signal: AbortSignal): Promise<VideoFrameResult<Blob>> {
  const result = await drawFirstFrame(url, signal);
  if (!result.ok) return result;
  const canvas = result.frame;
  try {
    // WebP keeps transparency; a browser that cannot write it gives PNG.
    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/webp"));
    return blob ? { ok: true, frame: blob } : { ok: false, failure: captureFailure() };
  } catch (error) {
    return { ok: false, failure: captureFailure(error) };
  } finally {
    canvas.width = canvas.height = 0;
  }
}

/** The playing video: its step, its file, its card's video group and frame, and whether it only plays while hovered. */
interface Playing { node: string; preview: CanvasVideoPreview; video: SVGGElement; frame: Element; hover: boolean }

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
  private readonly failed = new Map<string, VideoFailure>();
  /** Capture diagnostics remain separate: playback can work without readable pixels. */
  private readonly previewFailures = new Map<string, VideoFailure>();
  /** Source availability is recorded separately from browser decoding failures. */
  private readonly sourceFailures = new Map<string, VideoFailure>();
  /** Where each file stopped short of its end: hovering or playing it again continues there. */
  private readonly positions = new Map<string, number>();
  /** First frames waiting for their card to come into view. */
  private readonly waiting = new Map<Element, CanvasVideoPreview>();
  /** Reconsider a stationary pointer when its card's first frame becomes ready. */
  private readonly hoverRest = new WeakMap<SVGGElement, () => void>();
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

  constructor(private readonly host: HTMLElement, private readonly hoverPlays: () => boolean, private readonly covered: (element: Element) => boolean, private readonly onActivate: () => void = () => {}) {
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
      this.playbackFailed(mediaFailure(this.media.error));
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
  attach(card: SVGGElement, node: string, preview: CanvasVideoPreview, signal: AbortSignal) {
    const video = card.querySelector<SVGGElement>(".fx-canvas-video");
    const image = video?.querySelector(".fx-canvas-video-still");
    const frame = video?.querySelector(".fx-canvas-image-border");
    const play = video?.querySelector<SVGGElement>(".fx-canvas-play");
    if (!video || !image || !frame || !play) return;
    if (!preview.url) this.sourceFailures.set(preview.digest, failure(preview.unavailable_reason === "not_included" ? "omitted" : "unavailable"));
    else this.sourceFailures.delete(preview.digest);
    this.previewState(video, preview.digest);
    const poster = video.querySelector(".fx-canvas-video-poster");
    for (const event of ["load", "error"]) poster?.addEventListener(event, () => this.previewState(video, preview.digest), { signal });
    const still = this.stills.get(preview.digest);
    if (still) image.setAttribute("href", still);
    // A first frame already being read reaches this card when it is ready (read).
    else if (preview.url && !this.stills.has(preview.digest)) {
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
      if (!this.hoverPlays() || !this.interaction(preview.digest).hover) return;
      this.hoverTimer = window.setTimeout(() => {
        if (this.interaction(preview.digest).hover && video.isConnected
          && !(this.playing && (!this.playing.hover || this.playing.video === video))) this.play({ node, preview, video, frame }, true);
      }, HOVER_DELAY);
    };
    this.hoverRest.set(video, rest);
    signal.addEventListener("abort", () => this.hoverRest.delete(video), { once: true });
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
    this.queue.length = 0;
    for (const still of this.stills.values()) if (still) URL.revokeObjectURL(still);
    this.stills.clear();
    this.failed.clear();
    this.previewFailures.clear();
    this.sourceFailures.clear();
  }

  private play(playing: Omit<Playing, "hover">, hover = false) {
    const interaction = this.interaction(playing.preview.digest);
    if (!playing.preview.url || interaction.control === "unavailable" || (hover && (!this.hoverPlays() || !interaction.hover)) || this.covered(playing.video)) return;
    this.stop();
    this.onActivate();
    window.clearTimeout(this.hideTimer);
    window.clearTimeout(this.unloadTimer);
    this.playing = { ...playing, hover };
    this.player.setAttribute("aria-label", playing.preview.label);
    this.media.muted = hover;
    this.media.loop = hover;
    this.media.poster = playing.video.classList.contains("poster-failed") ? "" : playing.preview.poster?.url ?? "";
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
    void this.media.play().catch((error: unknown) => {
      // Autoplay permission and interrupted requests do not establish an unsupported file.
      if (this.playing?.video === playing.video && this.playing.preview.digest === playing.preview.digest
        && error instanceof DOMException && error.name === "NotSupportedError") {
        this.playbackFailed(this.media.error ? mediaFailure(this.media.error) : failure("source", error.message));
      }
    });
  }

  private playbackFailed(error: VideoFailure) {
    if (!this.playing) return;
    const { digest } = this.playing.preview;
    this.failed.set(digest, error);
    this.previewState(this.playing.video, digest);
    for (const video of this.host.querySelectorAll<SVGGElement>(`.fx-canvas-video[data-digest="${digest}"]`)) this.previewState(video, digest);
    this.stop();
    this.unload();
  }

  private interaction(digest: string) {
    return videoInteraction(this.stills.get(digest), this.sourceFailures.get(digest) ?? this.previewFailures.get(digest), this.failed.get(digest));
  }

  /** Native load failures are inert; readable native frames with failed capture may be tried manually. */
  private previewState(video: SVGGElement, digest: string) {
    const noStill = this.stills.get(digest) === null;
    const { control } = this.interaction(digest);
    video.classList.toggle("is-unavailable", control === "unavailable");
    video.classList.toggle("is-retry", control === "try");
    video.classList.toggle("is-preview-unavailable", noStill);
    const play = video.querySelector<SVGGElement>(".fx-canvas-play");
    if (play) {
      const label = `${control === "try" ? "Try playback" : "Play"} ${play.dataset.label ?? "video"}`;
      play.setAttribute("aria-label", label);
      play.setAttribute("aria-disabled", String(control === "unavailable"));
      play.setAttribute("tabindex", control === "unavailable" ? "-1" : "0");
      const title = play.querySelector("title");
      if (title) title.textContent = label;
    }
    const message = video.querySelector(".fx-canvas-image-error");
    const error = this.sourceFailures.get(digest) ?? this.failed.get(digest) ?? this.previewFailures.get(digest);
    if (!message || !error) return;
    const { reason, details } = videoFailureText(error);
    const label = videoFailureLabel(video.classList.contains("has-poster"), error);
    message.textContent = label;
    const line = document.createElementNS("http://www.w3.org/2000/svg", "tspan");
    line.setAttribute("x", message.getAttribute("x") ?? "0");
    line.setAttribute("dy", "16");
    line.textContent = reason;
    message.append(line);
    let title = video.querySelector(".fx-canvas-video-error-detail");
    if (!title) {
      title = document.createElementNS("http://www.w3.org/2000/svg", "title");
      title.classList.add("fx-canvas-video-error-detail");
      video.prepend(title);
    }
    title.textContent = `${label}. ${details}`;
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
  private read({ digest, url }: CanvasVideoPreview) {
    if (!url || this.stills.has(digest)) return;
    this.stills.set(digest, undefined);
    this.queue.push({ digest, url });
    this.next();
  }
  private next() {
    while (!this.abort.signal.aborted && this.loading < LOADS && this.queue.length) {
      const { digest, url } = this.queue.shift()!;
      this.loading++;
      void firstFrame(url, this.abort.signal).catch((error: unknown): VideoFrameResult<Blob> => ({ ok: false, failure: captureFailure(error) })).then((result) => {
        this.loading--;
        this.next();
        if (this.abort.signal.aborted) return;
        const still = result.ok ? URL.createObjectURL(result.frame) : null;
        if (!result.ok) this.previewFailures.set(digest, result.failure);
        this.stills.set(digest, still);
        // Whichever cards show the file now; a card drawn since the read began among them.
        for (const video of this.host.querySelectorAll<SVGGElement>(`.fx-canvas-video[data-digest="${digest}"]`)) {
          if (still) video.querySelector(".fx-canvas-video-still")?.setAttribute("href", still);
          this.previewState(video, digest);
          if (still && video.matches(":hover")) this.hoverRest.get(video)?.();
        }
      });
    }
  }
}
