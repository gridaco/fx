import { parsePetDemo, parseSpriteLayout, type SpriteLayout } from "../../lib/showcases/contracts";
import { loadShowcaseJson } from "../../lib/showcases/load";

type Row = SpriteLayout["rows"][number];
const DEFAULT_MS = 140;

export function durations(row: Row): number[] {
  return row.durations_ms.length === row.frames ? row.durations_ms : Array(row.frames).fill(DEFAULT_MS);
}

/** The frame a row shows `ms` into its loop, by its own frame timings. */
export function frameAt(row: Row, ms: number): number {
  const times = durations(row), cycle = times.reduce((total, time) => total + time, 0);
  let elapsed = ((ms % cycle) + cycle) % cycle;
  for (const [index, time] of times.entries()) {
    if (elapsed < time) return index;
    elapsed -= time;
  }
  return 0;
}

export function stateLabel(state: string): string {
  const words = state.replaceAll("-", " ");
  return words[0].toUpperCase() + words.slice(1);
}

/** Playback state without a document: which row, which frame, playing or paused. */
export class SpritePlayer {
  private index = 0;
  private elapsed = 0;
  playing: boolean;
  speed = 1;

  constructor(readonly layout: SpriteLayout, playing = true) { this.playing = playing; }

  get row(): Row { return this.layout.rows[this.index]; }
  get frame(): number { return frameAt(this.row, this.elapsed); }
  get loopMs(): number { return durations(this.row).reduce((total, time) => total + time, 0); }

  select(state: string) {
    const index = this.layout.rows.findIndex(row => row.state === state);
    if (index < 0) throw new Error(`Unknown sprite state: ${state}.`);
    this.index = index;
    this.elapsed = 0;
  }

  /** Advances by wall-clock time; returns whether the visible frame changed. */
  advance(ms: number): boolean {
    if (!this.playing) return false;
    const before = this.frame;
    this.elapsed = (this.elapsed + ms * this.speed) % this.loopMs;
    return this.frame !== before;
  }

  /** Steps one frame forward or back and pauses there. */
  step(offset: number) {
    const times = durations(this.row), next = (this.frame + offset + times.length) % times.length;
    this.playing = false;
    this.elapsed = times.slice(0, next).reduce((total, time) => total + time, 0);
  }

  /** The row a vertical position on the whole sheet falls in. */
  rowAt(fraction: number): Row {
    const index = Math.min(this.layout.rows.length - 1, Math.max(0, Math.floor(fraction * this.layout.rows.length)));
    return this.layout.rows[index];
  }
}

/** Plays one state of the recorded sheet at a time and marks its cell on the whole sheet. */
export class PetDemoController {
  private readonly events = new AbortController();
  private readonly canvas: HTMLCanvasElement;
  private readonly context: CanvasRenderingContext2D;
  private player: SpritePlayer | null = null;
  private sheet: HTMLImageElement | null = null;
  private mirrors: Record<string, string> = {};
  private animation = 0;
  private previous = 0;
  private visible = true;
  private observer: IntersectionObserver | null = null;
  private disposed = false;

  constructor(private readonly root: HTMLElement, private readonly assetBase: string) {
    this.canvas = this.element<HTMLCanvasElement>("canvas[data-pet]");
    this.context = this.canvas.getContext("2d")!;
    const signal = this.events.signal;
    this.element<HTMLButtonElement>("[data-play]").addEventListener("click", () => {
      if (!this.player) return;
      this.player.playing = !this.player.playing;
      this.sync();
    }, { signal });
    this.element<HTMLButtonElement>("[data-step]").addEventListener("click", () => { this.player?.step(1); this.sync(); }, { signal });
    this.element<HTMLSelectElement>("[data-speed]").addEventListener("change", event => {
      if (this.player) this.player.speed = Number((event.target as HTMLSelectElement).value);
    }, { signal });
    for (const button of root.querySelectorAll<HTMLButtonElement>("button[data-background]")) {
      button.addEventListener("click", () => {
        this.element<HTMLElement>("[data-stage]").dataset.background = button.dataset.background;
        root.querySelectorAll("button[data-background]").forEach(item => item.setAttribute("aria-pressed", String(item === button)));
      }, { signal });
    }
    this.element<HTMLElement>("[data-sheet]").addEventListener("click", event => {
      if (!this.player) return;
      const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
      this.choose(this.player.rowAt((event.clientY - bounds.top) / bounds.height).state);
    }, { signal });
    this.canvas.addEventListener("keydown", event => {
      if (!this.player || (event.key !== "ArrowLeft" && event.key !== "ArrowRight")) return;
      event.preventDefault();
      this.player.step(event.key === "ArrowRight" ? 1 : -1);
      this.sync();
    }, { signal });
    void this.load().catch(error => this.status(error instanceof Error ? error.message : "This preview could not be loaded."));
  }

  private element<T extends Element>(selector: string): T {
    const element = this.root.querySelector<T>(selector);
    if (!element) throw new Error(`Missing demo element: ${selector}`);
    return element;
  }

  private status(message: string) {
    if (!this.disposed) this.element<HTMLElement>("[data-status]").textContent = message;
  }

  private async load() {
    const signal = this.events.signal;
    const [demo, layout] = await Promise.all([
      loadShowcaseJson("pet", this.assetBase, "manifest.json", parsePetDemo, signal),
      loadShowcaseJson("pet", this.assetBase, "assets/sprite.json", parseSpriteLayout, signal),
    ]);
    // The visible sheet is also the canvas's source: one CORS request for both.
    const sheet = this.element<HTMLImageElement>("[data-sheet] img");
    await sheet.decode();
    if (this.disposed) return;
    const { width, height } = layout.cell;
    if (sheet.naturalWidth !== layout.columns * width || sheet.naturalHeight !== layout.rows.length * height) throw new Error("The sprite sheet does not match its layout.");
    this.sheet = sheet;
    this.mirrors = demo.mirrors;
    this.player = new SpritePlayer(layout, !matchMedia("(prefers-reduced-motion: reduce)").matches);
    this.canvas.width = width * 2;
    this.canvas.height = height * 2;
    this.root.style.setProperty("--pet-columns", String(layout.columns));
    this.root.style.setProperty("--pet-rows", String(layout.rows.length));
    const states = this.element<HTMLElement>("[data-states]");
    states.replaceChildren(...layout.rows.map(row => {
      const button = document.createElement("button");
      button.type = "button";
      button.dataset.state = row.state;
      button.textContent = stateLabel(row.state);
      button.addEventListener("click", () => this.choose(row.state), { signal });
      return button;
    }));
    this.root.querySelectorAll<HTMLButtonElement | HTMLSelectElement>("[data-play], [data-step], [data-speed]").forEach(control => { control.disabled = false; });
    this.observer = new IntersectionObserver(entries => { this.visible = entries.some(entry => entry.isIntersecting); });
    this.observer.observe(this.root);
    this.status("");
    this.sync();
    this.animation = requestAnimationFrame(time => this.tick(time));
  }

  private choose(state: string) {
    this.player?.select(state);
    this.sync();
  }

  private tick(time: number) {
    const elapsed = this.previous ? Math.min(time - this.previous, 100) : 0;
    this.previous = time;
    if (this.visible && this.player?.advance(elapsed)) this.draw();
    this.animation = requestAnimationFrame(next => this.tick(next));
  }

  private sync() {
    const player = this.player;
    if (!player) return;
    const play = this.element<HTMLButtonElement>("[data-play]"), action = player.playing ? "Pause" : "Play";
    play.setAttribute("aria-label", action);
    play.title = action;
    this.element<HTMLElement>('[data-icon="pause"]').hidden = !player.playing;
    this.element<HTMLElement>('[data-icon="play"]').hidden = player.playing;
    this.root.querySelectorAll<HTMLButtonElement>("button[data-state]").forEach(button => button.setAttribute("aria-pressed", String(button.dataset.state === player.row.state)));
    const seconds = (player.loopMs / 1000).toFixed(2), source = this.mirrors[player.row.state];
    const mirrored = source ? ` · mirrored from ${stateLabel(source).toLowerCase()}` : "";
    this.element<HTMLOutputElement>("[data-selection]").textContent = `${stateLabel(player.row.state)} · ${player.row.frames} frames · ${seconds} s loop${mirrored}`;
    this.draw();
  }

  private draw() {
    const player = this.player, sheet = this.sheet;
    if (!player || !sheet) return;
    const { width, height } = player.layout.cell, row = player.row.row, frame = player.frame;
    this.context.clearRect(0, 0, this.canvas.width, this.canvas.height);
    this.context.imageSmoothingQuality = "high";
    this.context.drawImage(sheet, frame * width, row * height, width, height, 0, 0, this.canvas.width, this.canvas.height);
    const overview = this.element<HTMLElement>("[data-sheet]");
    overview.style.setProperty("--pet-column", String(frame));
    overview.style.setProperty("--pet-row", String(row));
    overview.dataset.ready = "true";
    this.element<HTMLElement>("[data-frame]").textContent = `Frame ${frame + 1} of ${player.row.frames}`;
  }

  dispose() {
    this.disposed = true;
    this.events.abort();
    this.observer?.disconnect();
    cancelAnimationFrame(this.animation);
  }
}
