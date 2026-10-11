import { parseDemo, parseFace, parseParallax, type ShowcaseSlug } from "../../lib/showcases/contracts";
import { loadShowcaseJson } from "../../lib/showcases/load";
import { parallaxGuideLines, validateParallaxGuides, type ParallaxGuideLayer } from "./parallax-guides";

type Expression = { eyes: string; mouth: string };
type TimelineEntry = Expression & { duration_ms: number };
type PatchPixel = readonly [index: number, red: number, green: number, blue: number];

/** The exported helpers keep native patch composition testable without a browser. */
export function replacePatchRgb(target: Uint8ClampedArray, pixels: readonly PatchPixel[]) {
  for (const [index, red, green, blue] of pixels) {
    target[index] = red;
    target[index + 1] = green;
    target[index + 2] = blue;
  }
}

export function selectExpression(timeline: readonly TimelineEntry[], seconds: number): Expression {
  const duration = timeline.reduce((total, item) => total + item.duration_ms, 0);
  if (!duration || !timeline.length) return { eyes: "rest", mouth: "rest" };
  let elapsed = ((seconds * 1000 % duration) + duration) % duration;
  for (const item of timeline) {
    if (elapsed < item.duration_ms) return item;
    elapsed -= item.duration_ms;
  }
  return timeline[0];
}

abstract class MotionController {
  protected readonly events = new AbortController();
  protected frame = 0;
  protected disposed = false;
  private failureMessage: string | null = null;

  constructor(protected readonly root: HTMLElement, protected readonly assetBase: string) {
    for (const button of root.querySelectorAll<HTMLButtonElement>("button[data-background]")) {
      button.addEventListener("click", () => {
        this.element<HTMLElement>("[data-stage]").dataset.background = button.dataset.background;
        root.querySelectorAll("button[data-background]").forEach(item => item.setAttribute("aria-pressed", String(item === button)));
      }, { signal: this.events.signal });
    }
  }

  protected element<T extends Element>(selector: string): T {
    const element = this.root.querySelector<T>(selector);
    if (!element) throw new Error(`Missing demo element: ${selector}`);
    return element;
  }

  protected url(path: string) { return `${this.assetBase.replace(/\/$/, "")}/${path}`; }

  protected async json<T>(slug: ShowcaseSlug, path: string, parse: (value: unknown) => T): Promise<T> {
    return loadShowcaseJson(slug, this.assetBase, path, parse, this.events.signal);
  }

  protected async image(path: string): Promise<HTMLImageElement> {
    const image = new Image();
    image.crossOrigin = "anonymous";
    image.src = this.url(path);
    await image.decode();
    return image;
  }

  protected status(message: string) {
    if (this.failureMessage && message !== this.failureMessage) return;
    this.element<HTMLElement>("[data-status]").textContent = message;
  }

  protected failure(error: unknown) {
    if (!this.disposed) {
      this.failureMessage = error instanceof Error ? error.message : "This preview could not be loaded.";
      this.status(this.failureMessage);
    }
  }

  dispose() {
    this.disposed = true;
    this.events.abort();
    cancelAnimationFrame(this.frame);
    this.root.querySelectorAll("video").forEach(video => video.pause());
  }
}

type ParallaxManifest = {
  canvas: { width: number; height: number };
  layers: { id: string; file: string; offset_y: number; order: number; parallax: number }[];
};

export class ParallaxDemoController extends MotionController {
  private readonly canvas = this.element<HTMLCanvasElement>("canvas");
  private readonly context = this.canvas.getContext("2d")!;
  private layers: (ParallaxManifest["layers"][number] & { image: HTMLImageElement; pattern: CanvasPattern; visible: boolean })[] = [];
  private scroll = 0;
  private speed = 100;
  private playing = !matchMedia("(prefers-reduced-motion: reduce)").matches;
  private drag: { id: number; x: number } | null = null;
  private previous = 0;
  private guides: ReadonlyMap<string, ParallaxGuideLayer> = new Map();
  private showGuides = false;

  constructor(root: HTMLElement, assetBase: string, private readonly guideLayers: readonly ParallaxGuideLayer[] = []) {
    super(root, assetBase);
    const signal = this.events.signal;
    this.element<HTMLButtonElement>("[data-play]").addEventListener("click", () => {
      this.playing = !this.playing;
      this.syncPlayback();
    }, { signal });
    this.element<HTMLButtonElement>("[data-reset]").addEventListener("click", () => { this.scroll = 0; this.draw(); }, { signal });
    this.element<HTMLInputElement>("[data-speed]").addEventListener("input", event => {
      this.speed = Number((event.target as HTMLInputElement).value);
      this.element<HTMLOutputElement>("[data-speed-value]").textContent = `${this.speed} px/s`;
    }, { signal });
    this.root.querySelectorAll<HTMLInputElement>("[data-layer]").forEach(input => input.addEventListener("change", () => {
      const layer = this.layers.find(item => item.id === input.dataset.layer);
      if (layer) layer.visible = input.checked;
      this.draw();
    }, { signal }));
    const guideToggle = this.root.querySelector<HTMLInputElement>("[data-guides]");
    if (guideToggle) { guideToggle.checked = false; guideToggle.disabled = true; }
    const guideLegend = this.root.querySelector<HTMLElement>("[data-guide-legend]");
    if (guideLegend) guideLegend.hidden = true;
    guideToggle?.addEventListener("change", event => {
      this.showGuides = this.guides.size > 0 && (event.target as HTMLInputElement).checked;
      const legend = this.root.querySelector<HTMLElement>("[data-guide-legend]");
      if (legend) legend.hidden = !this.showGuides;
      this.draw();
    }, { signal });
    this.canvas.addEventListener("pointerdown", event => {
      if (event.button !== 0) return;
      this.drag = { id: event.pointerId, x: event.clientX };
      this.canvas.setPointerCapture(event.pointerId);
      this.canvas.style.cursor = "grabbing";
    }, { signal });
    this.canvas.addEventListener("pointermove", event => {
      if (!this.drag || this.drag.id !== event.pointerId) return;
      this.scroll -= (event.clientX - this.drag.x) * this.canvas.width / this.canvas.getBoundingClientRect().width;
      this.drag.x = event.clientX;
      this.draw();
    }, { signal });
    this.canvas.addEventListener("pointerup", event => {
      if (this.canvas.hasPointerCapture(event.pointerId)) this.canvas.releasePointerCapture(event.pointerId);
    }, { signal });
    this.canvas.addEventListener("lostpointercapture", () => { this.drag = null; this.canvas.style.cursor = "grab"; }, { signal });
    this.canvas.addEventListener("keydown", event => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      this.scroll += event.key === "ArrowRight" ? 100 : -100;
      this.draw();
    }, { signal });
    this.syncPlayback();
    void this.load().catch(error => this.failure(error));
  }

  private async load() {
    const manifest = await this.json("parallax", "manifest.json", parseParallax);
    const layers = await Promise.all(manifest.layers.map(async spec => {
      const image = await this.image(spec.file);
      const pattern = this.context.createPattern(image, "repeat-x");
      if (!pattern) throw new Error("This preview could not prepare its repeating layers.");
      return { ...spec, image, pattern, visible: true };
    }));
    if (this.disposed) return;
    this.canvas.width = manifest.canvas.width;
    this.canvas.height = manifest.canvas.height;
    this.layers = layers.sort((a, b) => a.order - b.order);
    let guideFailure: string | null = null;
    try {
      this.guides = validateParallaxGuides(this.guideLayers, layers.map(layer => ({
        id: layer.id, file: layer.file, width: layer.image.naturalWidth, height: layer.image.naturalHeight,
      })));
      const toggle = this.root.querySelector<HTMLInputElement>("[data-guides]");
      if (toggle) toggle.disabled = !this.guides.size;
    } catch (error) {
      guideFailure = `Join guides unavailable: ${error instanceof Error ? error.message : "Layer geometry does not match."}`;
    }
    this.root.querySelectorAll<HTMLButtonElement>("[data-play], [data-reset]").forEach(button => { button.disabled = false; });
    this.root.querySelectorAll<HTMLInputElement>("[data-layer]").forEach(input => { input.disabled = false; });
    this.status(guideFailure ?? "Drag the scene to move through its layers.");
    this.draw();
    this.frame = requestAnimationFrame(time => this.tick(time));
  }

  private syncPlayback() { this.element<HTMLButtonElement>("[data-play]").textContent = this.playing ? "Pause" : "Play"; }

  private draw() {
    this.context.clearRect(0, 0, this.canvas.width, this.canvas.height);
    for (const layer of this.layers) {
      if (!layer.visible) continue;
      const offset = (-this.scroll * layer.parallax) % layer.image.naturalWidth;
      // One periodic surface avoids separately antialiased tile edges at fractional offsets.
      layer.pattern.setTransform({ a: 1, b: 0, c: 0, d: 1, e: offset, f: layer.offset_y });
      this.context.fillStyle = layer.pattern;
      this.context.fillRect(0, layer.offset_y, this.canvas.width, layer.image.naturalHeight);
    }
    // Draw after the whole composite so opaque foreground layers cannot cover the guides.
    if (!this.showGuides) return;
    const placements = this.layers.flatMap(layer => {
      const guide = this.guides.get(layer.id);
      return guide ? [{ guide, offset_y: layer.offset_y, parallax: layer.parallax, visible: layer.visible }] : [];
    });
    const lines = parallaxGuideLines(placements, this.scroll, this.canvas, this.showGuides);
    if (!lines.length) return;
    this.context.save();
    this.context.strokeStyle = "#ef4444";
    this.context.lineWidth = 3;
    this.context.shadowColor = "#fff";
    this.context.shadowBlur = 2;
    for (const line of lines) {
      this.context.save();
      this.context.beginPath();
      this.context.rect(0, line.top, this.canvas.width, line.bottom - line.top);
      this.context.clip();
      this.context.setLineDash(line.kind === "join" ? [12, 8] : []);
      this.context.beginPath();
      this.context.moveTo(line.x, line.top);
      this.context.lineTo(line.x, line.bottom);
      this.context.stroke();
      this.context.restore();
    }
    this.context.restore();
  }

  private tick(now: number) {
    const delta = this.previous ? Math.min((now - this.previous) / 1000, .1) : 0;
    this.previous = now;
    if (this.playing && !this.drag && !document.hidden && this.speed) {
      this.scroll += delta * this.speed;
      this.draw();
    }
    this.frame = requestAnimationFrame(time => this.tick(time));
  }
}

export class MovieDemoController extends MotionController {
  private readonly video = this.element<HTMLVideoElement>("[data-movie]");
  private readonly original = this.element<HTMLVideoElement>("[data-original]");
  private readonly scrub = this.element<HTMLInputElement>("[data-scrub]");
  private fallback = false;
  private comparing = false;

  constructor(root: HTMLElement, assetBase: string) {
    super(root, assetBase);
    const signal = this.events.signal;
    this.video.addEventListener("loadedmetadata", () => { this.scrub.max = String(this.video.duration); this.status(""); this.syncTime(); }, { signal });
    this.video.addEventListener("play", () => {
      this.element<HTMLButtonElement>("[data-play]").textContent = "Pause";
      if (this.comparing) void this.original.play().catch(() => {});
    }, { signal });
    this.video.addEventListener("pause", () => {
      this.element<HTMLButtonElement>("[data-play]").textContent = "Play";
      this.original.pause();
    }, { signal });
    this.video.addEventListener("error", () => this.setFormat(true), { signal });
    this.video.addEventListener("timeupdate", () => this.syncTime(), { signal });
    this.video.addEventListener("seeked", () => this.syncOriginal(), { signal });
    this.original.addEventListener("loadedmetadata", () => this.syncOriginal(), { signal });
    this.element<HTMLButtonElement>("[data-play]").addEventListener("click", () => {
      if (this.video.paused) void this.video.play().catch(error => this.failure(error)); else this.video.pause();
    }, { signal });
    this.element<HTMLButtonElement>("[data-reset]").addEventListener("click", () => { this.video.currentTime = 0; this.syncOriginal(); }, { signal });
    this.scrub.addEventListener("input", () => { this.video.currentTime = Number(this.scrub.value); }, { signal });
    this.element<HTMLSelectElement>("[data-speed]").addEventListener("change", event => {
      this.video.playbackRate = Number((event.target as HTMLSelectElement).value);
      this.original.playbackRate = this.video.playbackRate;
    }, { signal });
    this.element<HTMLSelectElement>("[data-format]").addEventListener("change", event => this.setFormat((event.target as HTMLSelectElement).value === "image"), { signal });
    this.element<HTMLInputElement>("[data-compare]").addEventListener("change", event => {
      this.comparing = (event.target as HTMLInputElement).checked;
      this.element<HTMLElement>("[data-original-stage]").hidden = !this.comparing;
      this.element<HTMLElement>("[data-movie-stages]").dataset.comparing = String(this.comparing);
      this.syncOriginal();
      if (this.comparing && !this.video.paused && !this.fallback) void this.original.play().catch(() => {}); else this.original.pause();
    }, { signal });
    if (this.video.readyState >= 1) { this.scrub.max = String(this.video.duration); this.status(""); this.syncTime(); }
    void this.json("movie-sprite", "manifest.json", value => parseDemo("movie-sprite", value)).then(() => {
      if (!this.disposed && !matchMedia("(prefers-reduced-motion: reduce)").matches) void this.video.play().catch(() => {});
    }).catch(error => {
      this.video.pause(); this.original.pause(); this.failure(error);
      this.root.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLSelectElement>("button, input, select").forEach(control => { control.disabled = true; });
    });
  }

  private syncTime() {
    this.scrub.value = String(this.video.currentTime);
    this.element<HTMLOutputElement>("[data-time]").textContent = `${this.video.currentTime.toFixed(2)} / ${(this.video.duration || 8).toFixed(2)} s`;
    if (this.comparing && Math.abs(this.original.currentTime - this.video.currentTime) > .15) this.syncOriginal();
  }

  private syncOriginal() {
    if (this.original.readyState && Number.isFinite(this.original.duration)) this.original.currentTime = Math.min(this.video.currentTime, this.original.duration);
  }

  private setFormat(fallback: boolean) {
    this.fallback = fallback;
    this.element<HTMLSelectElement>("[data-format]").value = fallback ? "image" : "video";
    this.video.hidden = fallback;
    this.element<HTMLImageElement>("[data-fallback]").hidden = !fallback;
    this.root.querySelectorAll<HTMLInputElement | HTMLButtonElement | HTMLSelectElement>("[data-play], [data-reset], [data-scrub], [data-speed]").forEach(control => { control.disabled = fallback; });
    if (fallback) { this.video.pause(); this.original.pause(); } else void this.video.play().catch(() => {});
    this.status(fallback ? "Animated WebP loops automatically; seeking and speed controls use WebM." : "");
  }
}

type FaceManifest = {
  patch_application: string;
  feather_already_baked: boolean;
  source_size: [number, number];
  patch_size: [number, number];
  offset_xy: [number, number];
  timeline: TimelineEntry[];
  patches: { feature_id: string; key: string; state_id: string }[];
};

export class PortraitDemoController extends MotionController {
  private readonly canvas = this.element<HTMLCanvasElement>("[data-portrait]");
  private readonly context = this.canvas.getContext("2d", { willReadFrequently: true })!;
  private readonly zoom = this.element<HTMLCanvasElement>("[data-face-zoom]");
  private readonly zoomContext = this.zoom.getContext("2d")!;
  private readonly video = this.element<HTMLVideoElement>("[data-body]");
  private readonly scrub = this.element<HTMLInputElement>("[data-scrub]");
  private manifest!: FaceManifest;
  private canonical!: HTMLImageElement;
  private patches: { feature_id: string; state_id: string; pixels: PatchPixel[] }[] = [];
  private region!: { left: number; top: number; width: number; height: number };
  private mode = "face";
  private playing = !matchMedia("(prefers-reduced-motion: reduce)").matches;
  private automatic = true;
  private bodyAvailable = false;
  private duration = 8;
  private time = 0;
  private rate = 1;
  private previous = 0;
  private dirty = true;
  private renderedExpression: Expression | null = null;

  constructor(root: HTMLElement, assetBase: string) {
    super(root, assetBase);
    const signal = this.events.signal;
    this.video.addEventListener("error", () => {
      if (!this.bodyAvailable) return;
      this.bodyAvailable = false;
      this.setMode("face");
      this.bodyLayout(false);
      this.status("Body video is unavailable in this browser. Face controls remain available.");
      this.root.querySelectorAll<HTMLButtonElement>('[data-mode="body"], [data-mode="combined"]').forEach(button => { button.disabled = true; });
    }, { signal });
    this.element<HTMLButtonElement>("[data-play]").addEventListener("click", () => {
      this.playing = !this.playing;
      this.syncPlayback();
    }, { signal });
    this.element<HTMLButtonElement>("[data-reset]").addEventListener("click", () => {
      this.time = 0;
      if (this.video.readyState) this.video.currentTime = 0;
      this.dirty = true;
    }, { signal });
    this.element<HTMLSelectElement>("[data-speed]").addEventListener("change", event => {
      this.rate = Number((event.target as HTMLSelectElement).value);
      this.video.playbackRate = this.rate;
    }, { signal });
    this.scrub.addEventListener("input", () => {
      this.time = Number(this.scrub.value);
      if (this.video.readyState) this.video.currentTime = this.time;
      this.dirty = true;
    }, { signal });
    this.video.addEventListener("seeked", () => { this.dirty = true; }, { signal });
    this.root.querySelectorAll<HTMLButtonElement>("[data-mode]").forEach(button => button.addEventListener("click", () => this.setMode(button.dataset.mode!), { signal }));
    this.element<HTMLInputElement>("[data-automatic]").addEventListener("change", event => {
      this.automatic = (event.target as HTMLInputElement).checked;
      this.root.querySelectorAll<HTMLSelectElement>("[data-eyes], [data-mouth]").forEach(select => { select.disabled = this.automatic; });
      this.dirty = true;
    }, { signal });
    this.root.querySelectorAll<HTMLSelectElement>("[data-eyes], [data-mouth]").forEach(select => select.addEventListener("change", () => { this.dirty = true; }, { signal }));
    this.syncPlayback();
    void this.load().catch(error => {
      if (this.disposed) return;
      this.failure(error);
      this.canvas.hidden = true;
      this.element<HTMLImageElement>("[data-fallback]").hidden = true;
      this.root.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLSelectElement>("button, input, select").forEach(control => { control.disabled = true; });
    });
  }

  private async load() {
    const resources = await this.json("portrait-motion", "manifest.json", value => parseDemo("portrait-motion", value));
    const body = resources.includes("assets/body.webm") ? "assets/body.webm" : null;
    const [manifest, canonical] = await Promise.all([this.json("portrait-motion", "assets/face-manifest.json", parseFace), this.image("assets/canonical.png")]);
    if (manifest.patch_application !== "replace_selected_rgb_preserve_original_alpha" || !manifest.feather_already_baked) throw new Error("This preview uses an unsupported face patch format.");
    if (canonical.naturalWidth !== manifest.source_size[0] || canonical.naturalHeight !== manifest.source_size[1]) throw new Error("The canonical portrait dimensions do not match its manifest.");
    this.manifest = manifest;
    this.canonical = canonical;
    [this.canvas.width, this.canvas.height] = manifest.source_size;
    this.element<HTMLElement>("[data-stage]").style.setProperty("--portrait-aspect", `${this.canvas.width} / ${this.canvas.height}`);
    [this.zoom.width, this.zoom.height] = manifest.patch_size;
    const [left, top] = manifest.offset_xy;
    const [width, height] = manifest.patch_size;
    this.region = { left: Math.max(0, left), top: Math.max(0, top), width: Math.min(this.canvas.width, left + width) - Math.max(0, left), height: Math.min(this.canvas.height, top + height) - Math.max(0, top) };
    this.duration = manifest.timeline.reduce((total, item) => total + item.duration_ms, 0) / 1000;
    this.scrub.max = String(this.duration);
    this.patches = await Promise.all(manifest.patches.map(async record => {
      const image = await this.image(`assets/patches/${record.key}.png`);
      if (image.naturalWidth !== width || image.naturalHeight !== height) throw new Error("Face patch dimensions do not match their manifest.");
      const scratch = document.createElement("canvas");
      scratch.width = width; scratch.height = height;
      const context = scratch.getContext("2d", { willReadFrequently: true })!;
      context.drawImage(image, 0, 0);
      const data = context.getImageData(0, 0, width, height).data;
      const pixels: PatchPixel[] = [];
      for (let y = 0; y < this.region.height; y++) for (let x = 0; x < this.region.width; x++) {
        const source = ((y + this.region.top - top) * width + x + this.region.left - left) * 4;
        if (data[source + 3] !== 0 && data[source + 3] !== 255) throw new Error("Face patches must have binary support; blending is already baked into RGB.");
        if (data[source + 3] === 255) pixels.push([(y * this.region.width + x) * 4, data[source], data[source + 1], data[source + 2]]);
      }
      return { ...record, pixels };
    }));
    if (this.disposed) return;
    this.bodyAvailable = body !== null;
    this.bodyLayout(this.bodyAvailable);
    this.setMode(this.bodyAvailable ? "combined" : "face");
    if (body) {
      this.video.src = this.url(body);
      this.video.load();
    }
    this.status("");
    this.syncPlayback();
    this.frame = requestAnimationFrame(time => this.tick(time));
  }

  private syncPlayback() {
    this.element<HTMLButtonElement>("[data-play]").textContent = this.playing ? "Pause" : "Play";
    if (this.playing && this.bodyAvailable) void this.video.play().catch(() => {
      if (this.disposed || !this.bodyAvailable) return;
      this.playing = false;
      this.element<HTMLButtonElement>("[data-play]").textContent = "Play";
    }); else this.video.pause();
    this.dirty = true;
  }

  private setMode(mode: string) {
    this.mode = mode;
    this.root.querySelectorAll<HTMLButtonElement>("[data-mode]").forEach(button => button.setAttribute("aria-pressed", String(button.dataset.mode === mode)));
    this.dirty = true;
  }

  private bodyLayout(available: boolean) {
    this.element<HTMLElement>("[data-portrait-layout]").dataset.faceOnly = String(!available);
    this.element<HTMLElement>("[data-body-controls]").hidden = !available;
    this.element<HTMLElement>("[data-face-detail]").hidden = !available;
  }

  private draw() {
    const ctx = this.context;
    const selected = this.automatic ? selectExpression(this.manifest.timeline, this.time) : {
      eyes: this.element<HTMLSelectElement>("[data-eyes]").value,
      mouth: this.element<HTMLSelectElement>("[data-mouth]").value,
    };
    // Face expressions are discrete; native pixel readback is needed only when the image changes.
    const movingBody = this.mode !== "face" && this.bodyAvailable;
    if (this.dirty || movingBody || selected.eyes !== this.renderedExpression?.eyes || selected.mouth !== this.renderedExpression?.mouth) {
      ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);
      if (movingBody && this.video.readyState >= 2) ctx.drawImage(this.video, 0, 0); else ctx.drawImage(this.canonical, 0, 0);
      if (this.mode !== "body") {
        const { left, top, width, height } = this.region;
        const image = ctx.getImageData(left, top, width, height);
        for (const patch of this.patches) {
          const group = patch.feature_id === "mouth" ? "mouth" : "eyes";
          if (patch.state_id === selected[group]) replacePatchRgb(image.data, patch.pixels);
        }
        ctx.putImageData(image, left, top);
      }
      if (this.bodyAvailable) {
        const [left, top] = this.manifest.offset_xy;
        const [width, height] = this.manifest.patch_size;
        this.zoomContext.clearRect(0, 0, width, height);
        this.zoomContext.drawImage(this.canvas, left, top, width, height, 0, 0, width, height);
      }
      this.renderedExpression = selected;
    }
    const eyeLabels: Record<string, string> = { rest: "Open eyes", eyes_half: "Half blink", eyes_closed: "Closed eyes" };
    const mouthLabels: Record<string, string> = { rest: "Rest", mouth_smile: "Smile", mouth_a: "Open A" };
    this.element<HTMLOutputElement>("[data-selection]").textContent = this.mode === "body" ? "Original rest expression" : `${eyeLabels[selected.eyes]} · ${mouthLabels[selected.mouth]}`;
    this.scrub.value = String(this.time);
    this.element<HTMLOutputElement>("[data-time]").textContent = `${this.time.toFixed(2)} / ${this.duration.toFixed(2)} s`;
  }

  private tick(now: number) {
    const delta = this.previous ? Math.min((now - this.previous) / 1000, .1) : 0;
    this.previous = now;
    if (!document.hidden && (this.playing || this.dirty)) {
      if (this.playing) this.time = (this.time + delta * this.rate) % this.duration;
      if (this.bodyAvailable && this.video.readyState >= 2) this.time = this.video.currentTime;
      this.draw();
      this.dirty = false;
    }
    this.frame = requestAnimationFrame(time => this.tick(time));
  }
}
