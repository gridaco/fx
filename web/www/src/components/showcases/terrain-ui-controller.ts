import { parseDemo, parseSheet, parseTerrainLookup } from "../../lib/showcases/contracts";
import { loadShowcaseJson } from "../../lib/showcases/load";

type Grid = number[][];
type TerrainPreset = "meadow" | "islands" | "cavern";
type SheetRole = "panel_frame" | "button_rect" | "preview_icons";
interface Rect { x: number; y: number; width: number; height: number }
interface Insets { top: number; right: number; bottom: number; left: number }
interface SheetCell { cell: Rect; state?: string; glyph?: string }
interface SheetLayout { cells: SheetCell[]; insets?: Insets; draw_scale: number; band_fill?: "tile" | "stretch" }
interface Sheet { image: HTMLImageElement; layout: SheetLayout }

const WIDTH = 40, HEIGHT = 12, ATLAS_CELL = 120;

function need<T extends Element>(root: HTMLElement, selector: string): T {
  const element = root.querySelector<T>(selector);
  if (!element) throw new Error(`Missing showcase element: ${selector}`);
  return element;
}

function asset(base: string, path: string): string { return `${base.replace(/\/+$/, "")}/${path}`; }

function image(url: string, signal: AbortSignal): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const result = new Image();
    const clear = () => { result.onload = result.onerror = null; signal.removeEventListener("abort", abort); };
    const abort = () => { clear(); result.src = ""; reject(new DOMException("Aborted", "AbortError")); };
    if (signal.aborted) return abort();
    result.onload = () => { clear(); resolve(result); };
    result.onerror = () => { clear(); reject(new Error("The showcase image could not be loaded.")); };
    signal.addEventListener("abort", abort, { once: true });
    result.src = url;
  });
}

/** Diagonal neighbours count only when both adjacent cardinal neighbours are occupied. */
export function terrainMask(rows: readonly (readonly number[])[], x: number, y: number): string {
  const at = (a: number, b: number): number => +(rows[b]?.[a] === 1);
  const n = at(x, y - 1), e = at(x + 1, y), s = at(x, y + 1), w = at(x - 1, y);
  return [n & w & at(x - 1, y - 1), n, n & e & at(x + 1, y - 1), w, 1, e,
    s & w & at(x - 1, y + 1), s, s & e & at(x + 1, y + 1)].join("");
}

export function terrainPreset(preset: TerrainPreset): Grid {
  const rows = Array.from({ length: HEIGHT }, () => Array<number>(WIDTH).fill(0));
  if (preset === "islands") {
    for (const [x, y, width, height] of [[1, 6, 7, 2], [10, 3, 6, 3], [20, 5, 7, 2], [30, 2, 8, 3], [25, 10, 9, 2]]) {
      for (let yy = y; yy < y + height; yy++) for (let xx = x; xx < x + width; xx++) rows[yy][xx] = 1;
    }
    return rows;
  }
  for (let x = 0; x < WIDTH; x++) {
    const top = preset === "meadow" ? 7 + Math.round(Math.sin(x * .42) * 1.3) : 3;
    for (let y = top; y < HEIGHT; y++) rows[y][x] = 1;
  }
  if (preset === "meadow") {
    for (let x = 6; x < 11; x++) for (let y = 5; y < HEIGHT; y++) rows[y][x] = 1;
    for (let x = 22; x < 28; x++) for (let y = 4; y < HEIGHT; y++) rows[y][x] = 1;
    for (let x = 16; x < 20; x++) for (let y = 5; y < 9; y++) rows[y][x] = 0;
  } else {
    for (let y = 5; y < 9; y++) for (let x = 2; x < 38; x++) if (x % 10 < 7) rows[y][x] = 0;
    for (let x = 15; x < 20; x++) for (let y = 0; y < 4; y++) rows[y][x] = 1;
  }
  return rows;
}

/** Source and destination rectangles for the exact measured nine-slice, including partial repeat tiles. */
export function* nineSlicePieces(cell: Rect, insets: Insets, width: number, height: number, scale: number, tiled: boolean): Generator<{ source: Rect; destination: Rect }> {
  const sx = [cell.x, cell.x + insets.left, cell.x + cell.width - insets.right, cell.x + cell.width];
  const sy = [cell.y, cell.y + insets.top, cell.y + cell.height - insets.bottom, cell.y + cell.height];
  const dx = [0, insets.left / scale, width - insets.right / scale, width];
  const dy = [0, insets.top / scale, height - insets.bottom / scale, height];
  for (let row = 0; row < 3; row++) for (let column = 0; column < 3; column++) {
    const sw = sx[column + 1] - sx[column], sh = sy[row + 1] - sy[row];
    const dw = dx[column + 1] - dx[column], dh = dy[row + 1] - dy[row];
    if (dw <= 0 || dh <= 0) continue;
    const repeat = tiled && (row === 1 || column === 1);
    const tw = repeat && column === 1 ? sw / scale : dw;
    const th = repeat && row === 1 ? sh / scale : dh;
    for (let y = 0; y < dh; y += th) for (let x = 0; x < dw; x += tw) {
      const w = Math.min(tw, dw - x), h = Math.min(th, dh - y);
      yield { source: { x: sx[column], y: sy[row], width: sw * w / tw, height: sh * h / th },
        destination: { x: dx[column] + x, y: dy[row] + y, width: w, height: h } };
    }
  }
}

abstract class DemoController {
  protected readonly abort = new AbortController();
  protected readonly resize: ResizeObserver;
  protected frame = 0;
  protected loaded = false;

  constructor(protected readonly root: HTMLElement, protected readonly assetBase: string) {
    this.resize = new ResizeObserver(() => this.schedule());
  }

  dispose(): void {
    this.abort.abort();
    this.resize.disconnect();
    cancelAnimationFrame(this.frame);
  }

  protected schedule = (): void => {
    if (this.frame || !this.loaded || this.abort.signal.aborted) return;
    this.frame = requestAnimationFrame(() => { this.frame = 0; this.draw(); });
  };

  protected ready(): void {
    if (this.abort.signal.aborted) return;
    this.loaded = true;
    need<HTMLFieldSetElement>(this.root, "[data-controls]").disabled = false;
    this.root.dataset.ready = "true";
    this.draw();
  }

  protected failed = (): void => {
    if (this.abort.signal.aborted) return;
    need<HTMLElement>(this.root, "[data-status]").textContent = "This demo could not load. Refresh to try again.";
  };

  protected abstract draw(): void;
}

/** Owns the editable tile grid; React only mounts the existing generated atlas and controls. */
export class TerrainDemoController extends DemoController {
  private readonly canvas: HTMLCanvasElement;
  private readonly context: CanvasRenderingContext2D;
  private atlas?: HTMLImageElement;
  private lookup: Record<string, [number, number]> = {};
  private rows = terrainPreset("meadow");
  private preset: TerrainPreset = "meadow";
  private erase = false;
  private dragging = false;
  private grid = false;
  private camera = 0;
  private cell = 1;
  private offset = 0;
  private hover: [number, number] | null = null;
  private last: [number, number] | null = null;

  constructor(root: HTMLElement, assetBase: string) {
    super(root, assetBase);
    this.canvas = need(root, "[data-terrain-canvas]");
    this.context = this.canvas.getContext("2d")!;
    const options = { signal: this.abort.signal };
    root.addEventListener("click", this.click, options);
    root.addEventListener("input", this.input, options);
    this.canvas.addEventListener("pointerdown", this.pointerDown, options);
    this.canvas.addEventListener("pointermove", this.pointerMove, options);
    for (const event of ["pointerup", "pointercancel", "lostpointercapture"]) this.canvas.addEventListener(event, this.endStroke, options);
    this.canvas.addEventListener("pointerleave", () => { if (!this.dragging) { this.hover = null; this.schedule(); } }, options);
    this.resize.observe(this.canvas);
    void this.load().catch(this.failed);
  }

  private async load(): Promise<void> {
    await loadShowcaseJson("terrain", this.assetBase, "manifest.json", value => parseDemo("terrain", value), this.abort.signal);
    const [atlas, data] = await Promise.all([
      image(asset(this.assetBase, "assets/atlas.png"), this.abort.signal),
      loadShowcaseJson("terrain", this.assetBase, "assets/lookup.json", parseTerrainLookup, this.abort.signal),
    ]);
    const lookup = data.lookup;
    if (!lookup || Object.keys(lookup).length !== 47 || atlas.naturalWidth !== 1440 || atlas.naturalHeight !== 480 ||
      Object.entries(lookup).some(([mask, value]) => !/^[01]{9}$/.test(mask) || !Array.isArray(value) || value.length !== 2 ||
        !value.every(Number.isInteger) || value[0] < 0 || value[0] >= 12 || value[1] < 0 || value[1] >= 4)) {
      throw new Error("Invalid terrain atlas.");
    }
    this.atlas = atlas;
    this.lookup = lookup;
    this.ready();
  }

  private click = (event: MouseEvent): void => {
    if (!(event.target instanceof Element)) return;
    const button = event.target.closest<HTMLButtonElement>("button");
    if (!button || !this.loaded) return;
    const preset = button.dataset.preset;
    if (preset === "meadow" || preset === "islands" || preset === "cavern") {
      this.preset = preset;
      this.rows = terrainPreset(preset);
      this.hover = null;
      for (const item of this.root.querySelectorAll<HTMLElement>("[data-preset]")) item.setAttribute("aria-pressed", String(item === button));
    }
    if (button.hasAttribute("data-reset")) this.rows = terrainPreset(this.preset);
    if (button.hasAttribute("data-tool")) {
      this.erase = button.dataset.tool === "erase";
      for (const item of this.root.querySelectorAll<HTMLElement>("[data-tool]")) item.setAttribute("aria-pressed", String(item === button));
    }
    this.schedule();
  };

  private input = (event: Event): void => {
    const input = event.target;
    if (!(input instanceof HTMLInputElement)) return;
    if (input.hasAttribute("data-grid")) this.grid = input.checked;
    if (input.hasAttribute("data-camera")) this.camera = Number(input.value) / 100;
    this.schedule();
  };

  private position(event: PointerEvent): [number, number] {
    const rect = this.canvas.getBoundingClientRect();
    return [Math.floor((event.clientX - rect.left + this.offset) / this.cell), Math.floor((event.clientY - rect.top) / this.cell)];
  }

  private pointerDown = (event: PointerEvent): void => {
    if (!this.loaded || event.button !== 0) return;
    event.preventDefault();
    this.dragging = true;
    this.last = null;
    this.canvas.setPointerCapture(event.pointerId);
    this.pointerMove(event);
  };

  private pointerMove = (event: PointerEvent): void => {
    const [x, y] = this.position(event);
    if (x < 0 || x >= WIDTH || y < 0 || y >= HEIGHT) { this.hover = null; this.schedule(); return; }
    this.hover = [x, y];
    if (this.dragging) {
      const [x0, y0] = this.last ?? [x, y], steps = Math.max(Math.abs(x - x0), Math.abs(y - y0), 1);
      for (let step = 0; step <= steps; step++) {
        const xx = Math.round(x0 + (x - x0) * step / steps), yy = Math.round(y0 + (y - y0) * step / steps);
        this.rows[yy][xx] = this.erase || event.shiftKey ? 0 : 1;
      }
      this.last = [x, y];
    }
    this.schedule();
  };

  private endStroke = (): void => { this.dragging = false; this.last = null; };

  protected draw(): void {
    if (!this.atlas) return;
    const { context: ctx, width, height } = canvasContext(this.canvas);
    this.cell = height / HEIGHT;
    this.offset = this.camera * Math.max(0, WIDTH * this.cell - width);
    ctx.fillStyle = "#f5f5f5";
    ctx.fillRect(0, 0, width, height);
    let count = 0;
    for (let y = 0; y < HEIGHT; y++) for (let x = 0; x < WIDTH; x++) {
      if (!this.rows[y][x]) continue;
      count++;
      const tile = this.lookup[terrainMask(this.rows, x, y)];
      if (!tile) continue;
      const left = x * this.cell - this.offset;
      if (left + this.cell < 0 || left > width) continue;
      ctx.drawImage(this.atlas, tile[0] * ATLAS_CELL, tile[1] * ATLAS_CELL, ATLAS_CELL, ATLAS_CELL, left, y * this.cell, this.cell + .3, this.cell + .3);
    }
    if (this.grid) {
      ctx.strokeStyle = "#17171722";
      ctx.lineWidth = .6;
      ctx.beginPath();
      for (let x = 0; x <= WIDTH; x++) { ctx.moveTo(x * this.cell - this.offset, 0); ctx.lineTo(x * this.cell - this.offset, height); }
      for (let y = 0; y <= HEIGHT; y++) { ctx.moveTo(0, y * this.cell); ctx.lineTo(width, y * this.cell); }
      ctx.stroke();
    }
    const highlight = need<HTMLElement>(this.root, "[data-atlas-highlight]");
    const tile = this.hover && this.rows[this.hover[1]][this.hover[0]] ? this.lookup[terrainMask(this.rows, ...this.hover)] : null;
    highlight.hidden = !tile;
    if (tile) Object.assign(highlight.style, { left: `${tile[0] / 12 * 100}%`, top: `${tile[1] / 4 * 100}%`, width: `${100 / 12}%`, height: "25%" });
    if (this.hover) {
      ctx.strokeStyle = this.erase ? "#b91c1c" : "#171717";
      ctx.lineWidth = 2;
      ctx.strokeRect(this.hover[0] * this.cell - this.offset + 1, this.hover[1] * this.cell + 1, this.cell - 2, this.cell - 2);
    }
    need<HTMLElement>(this.root, "[data-status]").textContent = `${count} tiles · 40 × 12 cells`;
  }
}

function canvasContext(canvas: HTMLCanvasElement): { context: CanvasRenderingContext2D; width: number; height: number } {
  const rect = canvas.getBoundingClientRect(), ratio = Math.min(window.devicePixelRatio || 1, 2);
  const width = Math.max(1, Math.round(rect.width * ratio)), height = Math.max(1, Math.round(rect.height * ratio));
  if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
  const context = canvas.getContext("2d")!;
  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  context.clearRect(0, 0, rect.width, rect.height);
  return { context, width: rect.width, height: rect.height };
}

function parseLayout(value: unknown, image: HTMLImageElement, role: SheetRole): SheetLayout {
  const layout = parseSheet(value);
  if (!layout || layout.role !== role || !Array.isArray(layout.cells) || !layout.cells.length ||
    !Number.isFinite(layout.draw_scale) || layout.draw_scale <= 0 || layout.cells.some(({ cell }) => !cell ||
      ![cell.x, cell.y, cell.width, cell.height].every(Number.isFinite) || cell.x < 0 || cell.y < 0 || cell.width <= 0 || cell.height <= 0 ||
      cell.x + cell.width > image.naturalWidth || cell.y + cell.height > image.naturalHeight)) throw new Error("Invalid UI sheet layout.");
  if (role !== "preview_icons") {
    const inset = layout.insets;
    if (!inset || ![inset.top, inset.right, inset.bottom, inset.left].every(n => Number.isFinite(n) && n >= 0) ||
      layout.cells.some(({ cell }) => cell.width <= inset.left + inset.right || cell.height <= inset.top + inset.bottom)) throw new Error("Invalid nine-slice insets.");
  }
  return layout;
}

/** Uses the run's registered cells and measured insets, without baking text into generated artwork. */
export class UiKitDemoController extends DemoController {
  private readonly sheets = new Map<SheetRole, Sheet>();
  private readonly panel: HTMLElement;
  private readonly stage: HTMLElement;
  private picked = "heart";
  private walks = 0;
  private resizing: { x: number; y: number; width: number; height: number } | null = null;

  constructor(root: HTMLElement, assetBase: string) {
    super(root, assetBase);
    this.panel = need(root, "[data-panel]");
    this.stage = need(root, "[data-kit-stage]");
    const options = { signal: this.abort.signal };
    root.addEventListener("click", this.click, options);
    root.addEventListener("change", this.change, options);
    const grip = need<HTMLButtonElement>(root, "[data-resize]");
    grip.addEventListener("pointerdown", this.resizeStart, options);
    grip.addEventListener("pointermove", this.resizeMove, options);
    for (const event of ["pointerup", "pointercancel", "lostpointercapture"]) grip.addEventListener(event, () => { this.resizing = null; }, options);
    grip.addEventListener("keydown", this.resizeKey, options);
    for (const button of root.querySelectorAll<HTMLButtonElement>("[data-painted-button]")) {
      const set = (state: string) => { button.dataset.state = state; this.schedule(); };
      button.addEventListener("pointerenter", () => set("hover"), options);
      button.addEventListener("pointerleave", () => set("normal"), options);
      button.addEventListener("pointerdown", () => set("pressed"), options);
      button.addEventListener("pointerup", () => set("hover"), options);
      button.addEventListener("pointercancel", () => set("normal"), options);
      button.addEventListener("focus", () => set("hover"), options);
      button.addEventListener("blur", () => set("normal"), options);
      button.addEventListener("keydown", event => { if (event.key === " " || event.key === "Enter") set("pressed"); }, options);
      button.addEventListener("keyup", event => { if (event.key === " " || event.key === "Enter") set("hover"); }, options);
    }
    this.resize.observe(this.panel);
    this.resize.observe(this.stage);
    void this.load().catch(this.failed);
  }

  private async load(): Promise<void> {
    await loadShowcaseJson("ui-kit", this.assetBase, "manifest.json", value => parseDemo("ui-kit", value), this.abort.signal);
    await Promise.all((["panel_frame", "button_rect", "preview_icons"] as const).map(async role => {
      const [art, value] = await Promise.all([
        image(asset(this.assetBase, `assets/${role}.png`), this.abort.signal),
        loadShowcaseJson("ui-kit", this.assetBase, `assets/${role}_validation.json`, parseSheet, this.abort.signal),
      ]);
      this.sheets.set(role, { image: art, layout: parseLayout(value, art, role) });
    }));
    if (this.abort.signal.aborted) return;
    const panelLayout = this.sheets.get("panel_frame")!.layout, inset = panelLayout.insets!, scale = panelLayout.draw_scale;
    // Content stays inside the measured safe frame even when the destination changes size.
    this.panel.style.setProperty("--panel-top", `${inset.top / scale}px`);
    this.panel.style.setProperty("--panel-right", `${inset.right / scale}px`);
    this.panel.style.setProperty("--panel-bottom", `${inset.bottom / scale}px`);
    this.panel.style.setProperty("--panel-left", `${inset.left / scale}px`);
    this.ready();
  }

  private click = (event: MouseEvent): void => {
    if (!(event.target instanceof Element) || !this.loaded) return;
    const button = event.target.closest<HTMLButtonElement>("button");
    if (!button || button.disabled) return;
    const size = button.dataset.size;
    if (size) {
      this.panel.style.width = size === "wide" ? "740px" : "540px";
      this.panel.style.height = size === "tall" ? "530px" : "440px";
      for (const item of this.root.querySelectorAll<HTMLElement>("[data-size]")) item.setAttribute("aria-pressed", String(item === button));
    }
    const choice = button.dataset.choice;
    if (choice) {
      this.picked = choice;
      for (const item of this.root.querySelectorAll<HTMLElement>("[data-choice]")) item.setAttribute("aria-pressed", String(item === button));
      need<HTMLElement>(this.root, "[data-selection]").textContent = `${button.textContent?.trim()} packed`;
      this.feedback("A small choice can change the whole afternoon.");
    }
    if (button.hasAttribute("data-primary")) this.feedback(`Walk ${++this.walks} begun — ${this.picked === "heart" ? "kindness" : this.picked === "star" ? "wonder" : "a way home"} packed for the trail.`);
    if (button.hasAttribute("data-secondary")) this.feedback("A quiet moment, a warm breeze, and nowhere to rush.");
    if (button.hasAttribute("data-close")) { this.panel.hidden = true; need<HTMLElement>(this.root, "[data-reopen]").hidden = false; }
    if (button.hasAttribute("data-reopen")) { this.panel.hidden = false; button.hidden = true; }
    if (button.dataset.inspectIcon) {
      for (const item of this.root.querySelectorAll<HTMLElement>("[data-inspect-icon]")) item.setAttribute("aria-pressed", String(item === button));
      need<HTMLElement>(this.root, "[data-icon-name]").textContent = button.dataset.inspectIcon.replaceAll("_", " ");
    }
    this.schedule();
  };

  private change = (event: Event): void => {
    if (!(event.target instanceof HTMLInputElement) || !event.target.hasAttribute("data-disable")) return;
    need<HTMLButtonElement>(this.root, "[data-primary]").disabled = event.target.checked;
    this.feedback(event.target.checked ? "The primary action is disabled." : "Your next small adventure starts here.");
    this.schedule();
  };

  private feedback(text: string): void { need<HTMLElement>(this.root, "[data-feedback]").textContent = text; }

  private resizeStart = (event: PointerEvent): void => {
    if (event.button !== 0 || !this.loaded) return;
    event.preventDefault();
    this.resizing = { x: event.clientX, y: event.clientY, width: this.panel.offsetWidth, height: this.panel.offsetHeight };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  };

  private resizeMove = (event: PointerEvent): void => {
    if (!this.resizing) return;
    this.setSize(this.resizing.width + event.clientX - this.resizing.x, this.resizing.height + event.clientY - this.resizing.y);
  };

  private resizeKey = (event: KeyboardEvent): void => {
    const delta = event.shiftKey ? 40 : 10;
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) return;
    event.preventDefault();
    this.setSize(this.panel.offsetWidth + (event.key === "ArrowRight" ? delta : event.key === "ArrowLeft" ? -delta : 0),
      this.panel.offsetHeight + (event.key === "ArrowDown" ? delta : event.key === "ArrowUp" ? -delta : 0));
  };

  private setSize(width: number, height: number): void {
    this.panel.style.width = `${Math.max(300, Math.min(this.stage.clientWidth - 32, width))}px`;
    this.panel.style.height = `${Math.max(380, Math.min(680, height))}px`;
    for (const item of this.root.querySelectorAll<HTMLElement>("[data-size]")) item.setAttribute("aria-pressed", "false");
    this.schedule();
  }

  private drawSheet(canvas: HTMLCanvasElement, role: "panel_frame" | "button_rect", state: string): void {
    const sheet = this.sheets.get(role)!;
    const entry = sheet.layout.cells.find(cell => cell.state === state) ?? sheet.layout.cells[0];
    const { context, width, height } = canvasContext(canvas);
    for (const { source: s, destination: d } of nineSlicePieces(entry.cell, sheet.layout.insets!, width, height, sheet.layout.draw_scale, sheet.layout.band_fill === "tile")) {
      context.drawImage(sheet.image, s.x, s.y, s.width, s.height, d.x, d.y, d.width, d.height);
    }
  }

  protected draw(): void {
    if (!this.panel.hidden) this.drawSheet(need(this.root, "[data-panel-art]"), "panel_frame", "default");
    const icons = this.sheets.get("preview_icons")!;
    for (const canvas of this.root.querySelectorAll<HTMLCanvasElement>("[data-icon]")) {
      const entry = icons.layout.cells.find(cell => cell.glyph === canvas.dataset.icon);
      if (!entry) continue;
      const { context, width, height } = canvasContext(canvas), rect = entry.cell;
      context.drawImage(icons.image, rect.x, rect.y, rect.width, rect.height, 0, 0, width, height);
    }
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[data-painted-button]")) {
      this.drawSheet(button.querySelector("canvas")!, "button_rect", button.disabled ? "disabled" : button.dataset.state ?? "normal");
    }
    for (const canvas of this.root.querySelectorAll<HTMLCanvasElement>("[data-button-state]")) this.drawSheet(canvas, "button_rect", canvas.dataset.buttonState!);
    need<HTMLElement>(this.root, "[data-status]").textContent = this.panel.hidden ? "Journal closed" : `${this.panel.offsetWidth} × ${this.panel.offsetHeight} · nine-slice`;
  }
}
