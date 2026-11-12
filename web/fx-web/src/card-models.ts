import { ArrowLeft, createElement, Maximize, Minimize } from "lucide";
import type { CanvasModelPreview } from "./graph";
import type { Artifact } from "./index";
import { ModelSourceController } from "./model";
import { ModelPreviewController } from "./model-view";
import { loadModelRenderer, type ModelElement } from "./model-renderer";

/** The same framing and lighting always represent the actual recorded model. */
const PROFILE = "neutral-25-75-640x360-v1";
const WIDTH = 640, HEIGHT = 360, CACHE_LIMIT = 64, TIMEOUT = 30_000;
const keyOf = (preview: CanvasModelPreview) => `${PROFILE}:${preview.digest}:${preview.size}`;
const artifactOf = (preview: CanvasModelPreview): Artifact => ({
  digest: preview.digest, kind: "model/gltf-binary", name: preview.name,
  size: preview.size, available: preview.url !== null, url: preview.url,
});

/** Visible stills stay intact; full overview views can fall back to explicit inspection. */
export function thumbnailCapacity(keys: Iterable<string>, visible: ReadonlySet<string>, limit = CACHE_LIMIT): { ready: boolean; evict?: string } {
  const cached = [...keys];
  if (cached.length < limit) return { ready: true };
  const evict = cached.find(key => !visible.has(key));
  return evict === undefined ? { ready: false } : { ready: true, evict };
}

/** Retain every current failure so a large failing graph cannot create a retry loop. */
export function thumbnailFailureEvictions(keys: Iterable<string>, current: ReadonlySet<string>, historyLimit = CACHE_LIMIT): string[] {
  const historical = [...keys].filter(key => !current.has(key));
  return historical.slice(0, Math.max(0, historical.length - historyLimit));
}

function aborted(): DOMException { return new DOMException("Model preview cancelled", "AbortError"); }

/** Reject immediately when a card is no longer eligible, including during renderer setup. */
function until<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(aborted());
  return new Promise((resolve, reject) => {
    const cancel = () => reject(aborted());
    signal.addEventListener("abort", cancel, { once: true });
    promise.then(resolve, reject).finally(() => signal.removeEventListener("abort", cancel));
  });
}

/**
 * One temporary renderer, never one per card. Invisible model-viewer scenes receive one
 * pre-render in the pinned renderer, sufficient to capture the paused first frame.
 */
async function thumbnail(preview: CanvasModelPreview, signal: AbortSignal): Promise<Blob> {
  const source = new ModelSourceController();
  const cleanup = () => source.dispose();
  signal.addEventListener("abort", cleanup, { once: true });
  let model: ModelElement | undefined;
  let holder: HTMLDivElement | undefined;
  try {
    await until(source.load(artifactOf(preview)), signal);
    const state = source.getSnapshot();
    if (state.phase !== "ready" || !state.url) throw new Error(state.error ?? "The recorded model is unavailable.");
    await until(loadModelRenderer(), signal);
    if (signal.aborted) throw aborted();
    holder = document.createElement("div");
    holder.className = "fx-model-thumbnail-renderer";
    holder.inert = true;
    Object.assign(holder.style, { position: "fixed", left: "-10000px", top: "0", width: `${WIDTH}px`, height: `${HEIGHT}px`, opacity: "0", pointerEvents: "none" });
    model = document.createElement("model-viewer") as ModelElement;
    model.setAttribute("aria-hidden", "true");
    model.setAttribute("loading", "eager"); model.setAttribute("reveal", "auto");
    model.setAttribute("interaction-prompt", "none");
    model.setAttribute("environment-image", "neutral"); model.setAttribute("tone-mapping", "neutral");
    model.setAttribute("shadow-intensity", "0"); model.setAttribute("camera-orbit", "25deg 75deg auto");
    Object.assign(model.style, { display: "block", width: `${WIDTH}px`, height: `${HEIGHT}px` });
    const loaded = new Promise<void>((resolve, reject) => {
      model!.addEventListener("load", () => resolve(), { once: true, signal });
      model!.addEventListener("error", () => reject(new Error("The browser could not render this model.")), { once: true, signal });
    });
    holder.append(model); document.body.append(holder);
    model.src = state.url;
    await until(loaded, signal);
    await until(model.updateComplete, signal);
    model.pause();
    await until(new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))), signal);
    const image = new Image(); image.src = model.toDataURL();
    await until(image.decode(), signal);
    const canvas = document.createElement("canvas"); canvas.width = WIDTH; canvas.height = HEIGHT;
    try {
      const context = canvas.getContext("2d");
      if (!context) throw new Error("The browser could not capture this model.");
      context.fillStyle = "#f7f7f8"; context.fillRect(0, 0, WIDTH, HEIGHT);
      context.drawImage(image, 0, 0, WIDTH, HEIGHT);
      const blob = await until(new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/webp")), signal);
      if (!blob) throw new Error("The browser could not capture this model.");
      return blob;
    } finally { canvas.width = canvas.height = 0; }
  } finally {
    signal.removeEventListener("abort", cleanup);
    model?.pause(); model?.removeAttribute("src"); holder?.remove(); source.dispose();
  }
}

interface Binding {
  node: string; preview: CanvasModelPreview; card: SVGGElement; group: SVGGElement;
  image: SVGImageElement; frame: SVGRectElement; open: SVGGElement; visible: boolean;
}
interface Active { binding: Binding; controller: ModelPreviewController }

/** Lazy thumbnails and one explicit, in-place model interaction, separate from graph selection. */
export class CardModels {
  readonly player = document.createElement("div");
  private readonly mount = document.createElement("div");
  private readonly fullscreenButton = document.createElement("button");
  private readonly abort = new AbortController();
  private readonly bindings = new Map<Element, Binding>();
  private readonly stills = new Map<string, string>();
  private readonly failures = new Map<string, string>();
  private readonly queue = new Map<string, CanvasModelPreview>();
  private readonly capacityBlocked = new Set<string>();
  private readonly observer: IntersectionObserver;
  private job?: { key: string; abort: AbortController };
  private active?: Active;
  private fullscreenRequest?: { active: Active; focus?: Binding };
  private following = 0;
  private box = "";

  get interacting(): boolean { return Boolean(this.active); }

  constructor(private readonly host: HTMLElement, private readonly covered: (element: Element) => boolean, private readonly onActivate: () => void = () => {}) {
    this.player.className = "fx-canvas-model-player"; this.player.hidden = true; this.player.tabIndex = -1;
    this.player.setAttribute("role", "group");
    this.mount.className = "fx-canvas-model-mount";
    const back = document.createElement("button"); back.type = "button"; back.className = "fx-canvas-model-control fx-canvas-model-back";
    back.setAttribute("aria-label", "Back to poster"); back.title = "Back to poster";
    back.append(createElement(ArrowLeft, { width: 14, height: 14, "aria-hidden": "true" }));
    back.addEventListener("click", () => this.stop(true), { signal: this.abort.signal });
    this.fullscreenButton.type = "button"; this.fullscreenButton.className = "fx-canvas-model-control fx-canvas-model-fullscreen";
    this.fullscreenButton.addEventListener("click", () => this.fullscreen(), { signal: this.abort.signal });
    this.player.append(this.mount, back, this.fullscreenButton);
    this.renderFullscreen();
    for (const type of ["pointerdown", "pointerup", "click", "dblclick", "wheel"]) this.player.addEventListener(type, event => event.stopPropagation(), { signal: this.abort.signal });
    document.addEventListener("keydown", event => {
      if (event.key !== "Escape" || !this.active || !this.host.contains(event.target as Node)) return;
      event.preventDefault(); event.stopPropagation();
      if (this.fullscreened) void this.exitFullscreen();
      else this.stop(true);
    }, { capture: true, signal: this.abort.signal });
    this.player.addEventListener("keydown", event => {
      if (event.key.toLowerCase() !== "f" || event.defaultPrevented || event.repeat || event.metaKey || event.ctrlKey || event.altKey
        || (event.target as Element).closest("input, textarea, select, [contenteditable]")) return;
      event.preventDefault(); event.stopPropagation(); this.fullscreen();
    }, { signal: this.abort.signal });
    document.addEventListener("fullscreenchange", () => {
      this.box = ""; this.renderFullscreen(); this.recheck();
    }, { signal: this.abort.signal });
    document.addEventListener("pointerdown", event => {
      const target = event.target as Element;
      if (!this.active || this.player.contains(target) || target.closest(".fx-canvas-model-open")) return;
      const card = target.closest("[data-node-id]");
      if (!this.host.contains(target) || (card && card !== this.active.binding.card)) this.stop();
    }, { capture: true, signal: this.abort.signal });
    document.addEventListener("visibilitychange", () => this.recheck(), { signal: this.abort.signal });
    this.observer = new IntersectionObserver(entries => {
      for (const entry of entries) {
        const binding = this.bindings.get(entry.target);
        if (!binding) continue;
        binding.visible = entry.isIntersecting;
        if (binding.visible) this.enqueue(binding);
      }
      this.recheck();
    }, { root: host });
  }

  attach(card: SVGGElement, node: string, preview: CanvasModelPreview, signal: AbortSignal): void {
    const group = card.querySelector<SVGGElement>(".fx-canvas-model");
    const image = group?.querySelector<SVGImageElement>(".fx-canvas-model-still");
    const frame = group?.querySelector<SVGRectElement>(".fx-canvas-image-border");
    const open = group?.querySelector<SVGGElement>(".fx-canvas-model-open");
    if (!group || !image || !frame || !open) return;
    const binding = { card, node, preview, group, image, frame, open, visible: false };
    this.bindings.set(frame, binding); this.paint(binding); this.observer.observe(frame);
    const activate = (event: Event) => {
      event.preventDefault(); event.stopPropagation();
      if (!this.canInspect(binding)) return;
      this.start(binding);
    };
    open.addEventListener("click", activate, { signal });
    open.addEventListener("keydown", event => { if (event.key === "Enter" || event.key === " ") activate(event); }, { signal });
    signal.addEventListener("abort", () => {
      this.observer.unobserve(frame); this.bindings.delete(frame);
      // redrawn() rebinds an active preview to its replacement card.
      queueMicrotask(() => this.recheck());
    }, { once: true });
  }

  redrawn(cards: ReadonlyMap<string, SVGGElement>): void {
    this.trimHistory();
    if (!this.active) return;
    const old = this.active.binding;
    const binding = [...this.bindings.values()].find(item => item.card === cards.get(old.node) && item.preview.digest === old.preview.digest && item.preview.url !== null);
    if (!binding) { this.stop(); return; }
    old.group.classList.remove("is-shown"); binding.group.classList.toggle("is-shown", this.player.classList.contains("is-shown"));
    this.active.binding = binding;
  }

  recheck(): void {
    for (const binding of this.bindings.values()) this.syncControl(binding);
    if (this.active && !this.activeEligible()) this.stop();
    const eligible = new Set([...this.bindings.values()].filter(binding => this.eligible(binding, false)).map(binding => keyOf(binding.preview)));
    if (this.job && !eligible.has(this.job.key)) this.job.abort.abort();
    for (const key of this.queue.keys()) if (!eligible.has(key)) this.queue.delete(key);
    for (const binding of this.bindings.values()) if (this.eligible(binding)) this.enqueue(binding);
    this.next();
  }

  stop(restoreFocus = false): void {
    const active = this.active;
    if (!active) return;
    const focused = this.player.contains(document.activeElement);
    const focus = focused || restoreFocus ? active.binding : undefined;
    if (this.fullscreenRequest?.active === active) this.fullscreenRequest.focus = focus;
    this.active = undefined; cancelAnimationFrame(this.following); this.following = 0; this.box = "";
    void this.exitFullscreen(focus);
    active.binding.group.classList.remove("is-shown"); active.controller.dispose();
    this.player.hidden = true; this.player.classList.remove("is-shown");
    if (focus) this.restoreFocus(focus);
  }

  dispose(): void {
    this.stop(); this.abort.abort(); this.job?.abort.abort(); this.observer.disconnect();
    this.bindings.clear(); this.queue.clear(); this.capacityBlocked.clear();
    for (const url of this.stills.values()) URL.revokeObjectURL(url);
    this.stills.clear(); this.failures.clear();
  }

  private eligible(binding: Binding, visible = true): boolean {
    if (!this.current(binding) || (visible && !binding.visible)) return false;
    const frame = binding.frame.getBoundingClientRect(), host = this.host.getBoundingClientRect();
    return frame.width > 0 && frame.height > 0 && frame.right > host.left && frame.left < host.right && frame.bottom > host.top && frame.top < host.bottom;
  }

  /** Fullscreen owns its viewport, while the recording still owns source availability. */
  private activeEligible(): boolean {
    if (!this.active || !this.current(this.active.binding)) return false;
    return this.fullscreened || this.fullscreenRequest?.active === this.active || this.eligible(this.active.binding, false);
  }

  private current(binding: Binding): boolean {
    return !document.hidden && Boolean(binding.preview.url) && binding.frame.isConnected && !this.covered(binding.group);
  }

  private get fullscreened(): boolean { return document.fullscreenElement === this.player; }

  /** The same mounted model retains its camera, clip and playback through size changes. */
  private fullscreen(): void {
    const active = this.active;
    if (!active || !document.fullscreenEnabled || typeof this.player.requestFullscreen !== "function") return;
    if (this.fullscreened) { void this.exitFullscreen(); return; }
    if (this.fullscreenRequest?.active === active) return;
    const request: { active: Active; focus?: Binding } = { active };
    this.fullscreenRequest = request;
    void this.player.requestFullscreen().then(async () => {
      // A replacement preview may have its own newer request. Do not tear that one down.
      if (this.fullscreenRequest === request && (this.active !== active || this.abort.signal.aborted)) await this.exitFullscreen(request.focus);
    }).catch(() => {}).finally(() => {
      if (this.fullscreenRequest === request) this.fullscreenRequest = undefined;
      if (!this.abort.signal.aborted) { this.renderFullscreen(); this.recheck(); }
    });
  }

  private async exitFullscreen(focus?: Binding): Promise<void> {
    if (!this.fullscreened) return;
    await document.exitFullscreen().catch(() => {});
    if (focus && !this.active && !this.abort.signal.aborted) this.restoreFocus(focus);
  }

  private restoreFocus(binding: Binding): void {
    const target = binding.open.isConnected && this.canInspect(binding) ? binding.open : this.host;
    target.focus({ preventScroll: true });
  }

  private renderFullscreen(): void {
    this.fullscreenButton.hidden = !document.fullscreenEnabled || typeof this.player.requestFullscreen !== "function";
    const label = this.fullscreened ? "Exit full screen" : "Full screen";
    if (this.fullscreenButton.getAttribute("aria-label") === label) return;
    this.fullscreenButton.setAttribute("aria-label", label); this.fullscreenButton.title = label;
    this.fullscreenButton.replaceChildren(createElement(this.fullscreened ? Minimize : Maximize, { width: 14, height: 14, "aria-hidden": "true" }));
  }

  private paint(binding: Binding): void {
    const { preview, group, image } = binding, key = keyOf(preview);
    // Availability belongs to the current recording. A page-local cache never reveals omitted bytes.
    const still = preview.url ? this.stills.get(key) : undefined;
    const blocked = this.capacityBlocked.has(key);
    const error = !preview.url ? preview.unavailable_reason === "not_included" ? "Model is not included in this recording" : "Source model is unavailable" : this.failures.get(key);
    if (still) { image.setAttribute("href", still); this.stills.delete(key); this.stills.set(key, still); }
    else image.removeAttribute("href");
    group.classList.toggle("has-still", Boolean(still)); group.classList.toggle("is-unavailable", Boolean(error));
    group.classList.toggle("is-source-unavailable", !preview.url);
    group.classList.toggle("is-preview-unavailable", Boolean(preview.url && error));
    group.classList.toggle("is-loading", !still && !error && !blocked);
    this.syncControl(binding);
    const text = group.querySelector<SVGTextElement>(".fx-canvas-model-error");
    if (text) { text.textContent = error ? "Model preview unavailable" : blocked ? "Activate to preview this model" : "Loading 3D preview…"; text.setAttribute("visibility", still ? "hidden" : "visible"); }
    let title = group.querySelector<SVGTitleElement>(".fx-canvas-model-detail");
    if (!title) { title = document.createElementNS("http://www.w3.org/2000/svg", "title"); title.classList.add("fx-canvas-model-detail"); group.prepend(title); }
    title.textContent = error ?? (blocked ? "Activate 3D to preview this model. Automatic thumbnails resume when more space is available." : preview.label);
  }

  private canInspect(binding: Binding): boolean {
    return this.eligible(binding, false);
  }

  private syncControl(binding: Binding): void {
    const enabled = this.canInspect(binding);
    binding.open.setAttribute("aria-disabled", String(!enabled));
    binding.open.setAttribute("tabindex", enabled ? "0" : "-1");
  }

  private enqueue(binding: Binding): void {
    const key = keyOf(binding.preview);
    if (this.abort.signal.aborted || !this.eligible(binding) || this.stills.has(key) || this.failures.has(key) || this.job?.key === key) return;
    this.queue.set(key, binding.preview);
  }

  private trimHistory(): void {
    const current = new Set([...this.bindings.values()].map(binding => keyOf(binding.preview)));
    for (const key of thumbnailFailureEvictions(this.failures.keys(), current)) this.failures.delete(key);
    for (const key of this.capacityBlocked) if (!current.has(key)) this.capacityBlocked.delete(key);
  }

  private makeRoom(): boolean {
    const visible = new Set([...this.bindings.values()].filter(binding => this.eligible(binding, false)).map(binding => keyOf(binding.preview)));
    const room = thumbnailCapacity(this.stills.keys(), visible);
    if (room.evict !== undefined) {
      URL.revokeObjectURL(this.stills.get(room.evict)!); this.stills.delete(room.evict);
      for (const binding of this.bindings.values()) if (keyOf(binding.preview) === room.evict) this.paint(binding);
    }
    return room.ready;
  }

  private next(): void {
    if (this.abort.signal.aborted || this.job || document.hidden || !this.queue.size) return;
    if (!this.makeRoom()) {
      for (const key of this.queue.keys()) this.capacityBlocked.add(key);
      for (const binding of this.bindings.values()) if (this.capacityBlocked.has(keyOf(binding.preview))) this.paint(binding);
      return;
    }
    for (const [key, preview] of this.queue) {
      this.queue.delete(key);
      if (this.stills.has(key) || this.failures.has(key) || ![...this.bindings.values()].some(binding => keyOf(binding.preview) === key && this.eligible(binding))) continue;
      this.capacityBlocked.delete(key);
      for (const binding of this.bindings.values()) if (keyOf(binding.preview) === key) this.paint(binding);
      const abort = new AbortController(); this.job = { key, abort };
      const timer = setTimeout(() => abort.abort("timeout"), TIMEOUT);
      void thumbnail(preview, abort.signal).then(blob => {
        if (abort.signal.aborted || this.abort.signal.aborted) return;
        this.stills.set(key, URL.createObjectURL(blob));
      }).catch(error => {
        if (this.abort.signal.aborted || (abort.signal.aborted && abort.signal.reason !== "timeout")) return;
        this.failures.set(key, abort.signal.reason === "timeout" ? "Loading the model timed out" : error instanceof Error ? error.message : "The browser could not preview this model");
        this.trimHistory();
      }).finally(() => {
        clearTimeout(timer); this.job = undefined;
        if (this.abort.signal.aborted) return;
        for (const binding of this.bindings.values()) if (keyOf(binding.preview) === key) this.paint(binding);
        this.recheck();
      });
      return;
    }
  }

  private start(binding: Binding): void {
    if (this.active?.binding === binding) return;
    this.stop(); this.onActivate();
    this.player.hidden = false; this.player.classList.remove("is-shown"); this.player.setAttribute("aria-label", `3D preview ${binding.preview.label}`);
    const reveal = () => {
      requestAnimationFrame(() => requestAnimationFrame(() => {
        if (this.active?.controller !== controller) return;
        this.active.binding.group.classList.add("is-shown"); this.player.classList.add("is-shown");
      }));
    };
    const controller = new ModelPreviewController(this.mount, artifactOf(binding.preview), { download: false, defer: false, onReady: reveal, onError: reveal });
    this.active = { binding, controller }; this.follow();
    this.player.focus({ preventScroll: true });
  }

  private follow = (): void => {
    if (!this.active) return;
    if (!this.activeEligible()) { this.stop(); return; }
    if (!this.fullscreened) {
      const frame = this.active.binding.frame.getBoundingClientRect(), host = this.host.getBoundingClientRect();
      const values = [frame.left - host.left, frame.top - host.top, frame.width, frame.height], box = values.join();
      if (box !== this.box) {
        this.box = box;
        (["left", "top", "width", "height"] as const).forEach((side, index) => { this.player.style[side] = `${values[index]}px`; });
      }
    }
    this.following = requestAnimationFrame(this.follow);
  };
}
