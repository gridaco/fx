import type { CanvasController, CanvasGraph, LayoutReport } from "@grida/fx-web";

type MotionPreference = Pick<MediaQueryList, "matches" | "addEventListener" | "removeEventListener">;
type ObserverFactory = (callback: IntersectionObserverCallback) => Pick<IntersectionObserver, "observe" | "disconnect">;
type Workflow = { graph: CanvasGraph; layout: LayoutReport | null };
/** A camera position: the graph point held at the view point, and the zoom. */
type Shot = { x: number; y: number; zoom: number };
/** A planned camera move through shots at given shares of its duration, and how far it has run. */
type Tour = { shots: Shot[]; times: number[]; duration: number; elapsed: number; container: HTMLElement; size: string };
type Buffer = { layer: HTMLElement; container: HTMLElement; canvas: CanvasController | null; tour: Tour | null };

/** How wide a card is on screen where the camera stops on it: readable, with its neighbours in view. */
const CLOSE_CARD_PX = 210;
const HOLD_EVENTS = ["pointerenter", "pointerleave", "focusin", "focusout"] as const;
/** How much closer a stop is than the opening shot, at most. */
const OPENING_SHARE = 3;
/** The canvas's own zoom limit. */
const MAX_ZOOM = 3;
/** Where the camera centres a card, as a share of the workflow card. */
const VIEW_POINT = { x: 0.5, y: 0.5 };

/**
 * Shows one workflow at a time as two cards: the result it made and its recorded graph. The
 * active segment's CSS fill is the clock: its `animationend` advances the reel, so pausing the
 * animation pauses the timing exactly.
 *
 * The graph is the showcase pages' canvas, loaded only when first needed, over a verified run
 * recording fetched one workflow ahead. Two canvases take turns: the next graph draws on the
 * hidden one, then they crossfade. Within each workflow the camera tours the run as the slide
 * plays: from the whole graph to its first pictures, across the middle, to the card holding the
 * result. The tour moves the canvas's own camera, so lines stay thin and labels fade as they do
 * in the viewer, and it pauses with the reel. The canvas never takes input; the card is a link. Data
 * saving draws no graphs; reduced motion holds on the result's card and never advances.
 */
export class LandingReelController {
  private index = 0;
  /** The pointer or focus is on the workflow card: hold this workflow while it is looked at. */
  private holding = false;
  private visible = true;
  private disposed = false;
  private front = 0;
  private frameRequest: number | null = null;
  private lastFrame: number | null = null;
  private readonly buffers: Buffer[];
  private readonly workflows = new Map<number, Promise<Workflow | null>>();
  private readonly abort = new AbortController();
  private readonly items: HTMLElement[];
  private readonly segments: HTMLButtonElement[];
  private readonly captions: HTMLElement[];
  private readonly card: HTMLElement | null;
  private readonly observer;
  private readonly preferenceChanged = () => this.sync();

  constructor(
    private readonly root: HTMLElement,
    private readonly motion: MotionPreference = matchMedia("(prefers-reduced-motion: reduce)"),
    observe: ObserverFactory = callback => new IntersectionObserver(callback),
  ) {
    this.items = Array.from(root.querySelectorAll<HTMLElement>("[data-reel-item]"));
    this.segments = Array.from(root.querySelectorAll<HTMLButtonElement>("[data-reel-segment]"));
    this.captions = Array.from(root.querySelectorAll<HTMLElement>("[data-reel-caption]"));
    this.card = root.querySelector<HTMLElement>("[data-reel-layer]")?.parentElement ?? null;
    this.buffers = Array.from(root.querySelectorAll<HTMLElement>("[data-reel-layer]"), layer => ({
      layer,
      container: layer.querySelector<HTMLElement>("[data-reel-canvas]")!,
      canvas: null,
      tour: null,
    }));
    if (this.lite()) root.dataset.lite = "true";
    this.observer = observe(entries => {
      this.visible = entries.some(entry => entry.isIntersecting);
      this.sync();
    });
    this.observer.observe(root);
    root.addEventListener("click", this.click);
    root.addEventListener("animationend", this.advance);
    for (const type of HOLD_EVENTS) this.card?.addEventListener(type, this.hold);
    document.addEventListener("visibilitychange", this.preferenceChanged);
    motion.addEventListener("change", this.preferenceChanged);
    this.show(0);
  }

  dispose(): void {
    this.disposed = true;
    this.abort.abort();
    this.observer.disconnect();
    this.root.removeEventListener("click", this.click);
    this.root.removeEventListener("animationend", this.advance);
    for (const type of HOLD_EVENTS) this.card?.removeEventListener(type, this.hold);
    document.removeEventListener("visibilitychange", this.preferenceChanged);
    this.motion.removeEventListener("change", this.preferenceChanged);
    for (const video of this.root.querySelectorAll("video")) video.pause();
    if (this.frameRequest !== null) cancelAnimationFrame(this.frameRequest);
    for (const buffer of this.buffers) buffer.canvas?.dispose();
  }

  private playing(): boolean {
    return this.visible && !this.motion.matches && document.visibilityState !== "hidden";
  }

  /** A visitor saving data gets the results alone. */
  private lite(): boolean {
    const connection = (navigator as Navigator & { connection?: { saveData?: boolean } }).connection;
    return connection?.saveData === true;
  }

  private show(index: number): void {
    this.index = (index + this.items.length) % this.items.length;
    const active = (list: HTMLElement[], at: number) => list.forEach((element, i) => {
      if (i === at) element.dataset.active = "true";
      else delete element.dataset.active;
    });
    active(this.items, this.index);
    active(this.segments, this.index);
    this.items.forEach((item, i) => item.setAttribute("aria-hidden", String(i !== this.index)));
    this.captions.forEach(caption => { caption.hidden = Number(caption.dataset.index) !== this.index; });
    this.segments.forEach((segment, i) => segment.setAttribute("aria-current", String(i === this.index)));
    // Restart the clock: the fill only animates under data-active, so re-adding it resets it.
    const fill = this.segments[this.index]?.querySelector<HTMLElement>("[data-reel-fill]");
    if (fill) { fill.style.animation = "none"; void fill.offsetWidth; fill.style.removeProperty("animation"); }
    if (!this.lite()) {
      void this.present(this.index);
      void this.workflow(this.index + 1);
    }
    this.sync();
  }

  private sync(): void {
    const playing = this.playing();
    // Holding stops the clock, not the tour: the workflow stays while it is looked at.
    this.root.dataset.state = playing && !this.holding ? "playing" : "paused";
    this.items.forEach((item, i) => {
      const video = item.querySelector<HTMLVideoElement>("video[data-src]");
      if (!video) return;
      if (i !== this.index || this.motion.matches) { video.pause(); return; }
      if (!video.getAttribute("src")) video.src = video.dataset.src!;
      // Autoplay may be refused; the poster remains a useful frame.
      if (playing) void video.play().catch(() => {});
      else video.pause();
    });
    if (playing && this.frameRequest === null) {
      this.lastFrame = null;
      this.frameRequest = requestAnimationFrame(this.tick);
    }
  }

  /** Advances the shown workflow's tour while the reel plays; the frame loop stops when it pauses. */
  private tick = (now: number): void => {
    this.frameRequest = null;
    if (this.disposed || !this.playing()) return;
    const buffer = this.buffers[this.front];
    const tour = buffer?.tour;
    // Aimed while the tour runs, and after it whenever the card changes size under it.
    if (buffer?.canvas && tour) {
      const size = `${tour.container.clientWidth}x${tour.container.clientHeight}`;
      if (tour.elapsed < tour.duration || size !== tour.size) {
        tour.elapsed = Math.min(tour.duration, tour.elapsed + (this.lastFrame === null ? 0 : now - this.lastFrame));
        tour.size = size;
        this.aim(buffer.canvas, tour, tour.elapsed / tour.duration);
      }
    }
    this.lastFrame = now;
    this.frameRequest = requestAnimationFrame(this.tick);
  };

  /** One verified recording per workflow, read once; a workflow without one keeps the last graph. */
  private workflow(index: number): Promise<Workflow | null> {
    const at = (index + this.items.length) % this.items.length;
    const cached = this.workflows.get(at);
    if (cached) return cached;
    const item = this.items[at];
    const url = item?.dataset.recordingUrl, sha256 = item?.dataset.recordingSha256;
    const loading = !url || !sha256 ? Promise.resolve(null) : import("@grida/fx-web")
      .then(async web => {
        const loaded = await web.readRunExport({ url, sha256 }, this.abort.signal);
        return { graph: web.exportCanvasGraph(loaded, null), layout: loaded.manifest.layout };
      })
      .catch(() => null);
    this.workflows.set(at, loading);
    return loading;
  }

  /** Draws the workflow on the hidden canvas, then crossfades to it. */
  private async present(index: number): Promise<void> {
    const workflow = await this.workflow(index);
    if (!workflow || this.disposed || index !== this.index) return;
    const back = this.buffers[1 - this.front];
    if (!back) return;
    if (!back.canvas) {
      const { CanvasController } = await import("@grida/fx-web");
      if (this.disposed || index !== this.index) return;
      back.canvas = new CanvasController(back.container, () => {});
    }
    back.tour = null;
    back.canvas.setGraph(workflow.graph, null, workflow.layout);
    back.canvas.fit();
    back.tour = this.tour(back, workflow.graph, this.items[index]?.dataset.recordingFocus);
    back.layer.dataset.active = "true";
    const previous = this.buffers[this.front];
    if (previous) delete previous.layer.dataset.active;
    this.front = 1 - this.front;
    this.sync();
  }

  /**
   * The camera's path over the fitted graph: the whole run, then its pictures from first to the
   * result's card, each close enough to read. It lasts one slide, so it ends as the reel moves on.
   */
  private tour(buffer: Buffer, graph: CanvasGraph, focus: string | undefined): Tour | null {
    const canvas = buffer.canvas!;
    const frame = buffer.container.getBoundingClientRect();
    if (!frame.width || !frame.height) return null;
    const fitted = canvas.getCamera();
    const view = { x: frame.width * VIEW_POINT.x, y: frame.height * VIEW_POINT.y };
    // Screen positions at the fitted camera, back to graph coordinates.
    const place = (id: string) => {
      const card = buffer.container.querySelector<SVGGElement>(`[data-node-id="${CSS.escape(id)}"]`)?.getBoundingClientRect();
      if (!card?.width) return null;
      const x = card.left - frame.left + card.width / 2, y = card.top - frame.top + card.height / 2;
      return { id, x: (x - fitted.x) / fitted.zoom, y: (y - fitted.y) / fitted.zoom, width: card.width };
    };
    const pictures = graph.nodes.filter(node => node.preview).map(node => place(node.id))
      .filter(item => item !== null).sort((a, b) => a.x - b.x || a.y - b.y);
    const result = pictures.find(item => graph.nodes.find(node => node.id === item.id)?.preview?.digest === focus) ?? pictures.at(-1);
    if (!result) return null;
    const rest = pictures.filter(item => item !== result);
    const stops = [rest[0], rest[Math.floor(rest.length / 2)], result].filter((item, at, all) => item && all.indexOf(item) === at) as typeof pictures;
    const close = (stop: (typeof pictures)[number]): Shot =>
      ({ x: stop.x, y: stop.y, zoom: Math.min(MAX_ZOOM, Math.max(fitted.zoom, fitted.zoom * CLOSE_CARD_PX / stop.width)) });
    // The opening shot: the whole run when that stays recognizable; otherwise a third of reading
    // size, the first pictures near the left edge, so a long run opens as a row of cards.
    const first = close(stops[0]);
    const opening: Shot = fitted.zoom >= first.zoom / OPENING_SHARE
      ? { x: (view.x - fitted.x) / fitted.zoom, y: (view.y - fitted.y) / fitted.zoom, zoom: fitted.zoom }
      : { x: stops[0].x + (view.x - frame.width * 0.22) / (first.zoom / OPENING_SHARE), y: stops[0].y, zoom: first.zoom / OPENING_SHARE };
    const shots = [opening, opening, ...stops.map(close), close(result)];
    const times = [0, 0.08, ...stops.map((_, at) => 0.08 + 0.78 * (at + 1) / stops.length), 1];
    const duration = parseFloat(getComputedStyle(this.root).getPropertyValue("--reel-duration")) * 1000 || 9000;
    const tour: Tour = { shots, times, duration, elapsed: 0, container: buffer.container, size: "" };
    // Reduced motion holds on the result.
    if (this.motion.matches) tour.elapsed = duration;
    this.aim(canvas, tour, tour.elapsed / duration);
    return tour;
  }

  /**
   * The tour's camera at `progress`: eased between shots, zooming evenly rather than linearly, and
   * centred in the card as it is now.
   */
  private aim(canvas: CanvasController, tour: Tour, progress: number): void {
    const { shots, times, container } = tour;
    const view = { x: container.clientWidth * VIEW_POINT.x, y: container.clientHeight * VIEW_POINT.y };
    let at = 1;
    while (at < times.length - 1 && progress > times[at]) at++;
    const from = shots[at - 1], to = shots[at];
    const span = times[at] - times[at - 1];
    const linear = span > 0 ? Math.min(1, Math.max(0, (progress - times[at - 1]) / span)) : 1;
    const t = linear < 0.5 ? 4 * linear ** 3 : 1 - (-2 * linear + 2) ** 3 / 2;
    const zoom = from.zoom * (to.zoom / from.zoom) ** t;
    const x = from.x + (to.x - from.x) * t, y = from.y + (to.y - from.y) * t;
    canvas.setCamera({ x: view.x - x * zoom, y: view.y - y * zoom, zoom });
  }

  private hold = (event: Event): void => {
    const card = event.currentTarget as HTMLElement;
    this.holding = event.type === "pointerenter" || event.type === "focusin"
      || (event.type === "focusout" && card.matches(":hover"))
      || (event.type === "pointerleave" && card.contains(document.activeElement));
    this.sync();
  };

  private advance = (event: AnimationEvent): void => {
    if (!(event.target instanceof HTMLElement) || !event.target.matches("[data-reel-fill]")) return;
    if (this.playing()) this.show(this.index + 1);
  };

  private click = (event: MouseEvent): void => {
    if (!(event.target instanceof Element)) return;
    const segment = event.target.closest<HTMLButtonElement>("[data-reel-segment]");
    if (segment) this.show(this.segments.indexOf(segment));
  };
}
