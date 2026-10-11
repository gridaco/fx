/** The native range owns pointer and keyboard behavior; this class owns presentation. */
export class ImageCompareController {
  private readonly range: HTMLInputElement;

  constructor(private readonly root: HTMLElement, initial = 50) {
    const range = root.querySelector<HTMLInputElement>("[data-compare-range]");
    if (!range) throw new Error("Image comparison is missing its range control.");
    this.range = range;
    this.range.addEventListener("input", this.update);
    this.range.value = String(comparisonPosition(initial));
    this.update();
  }

  dispose() {
    this.range.removeEventListener("input", this.update);
  }

  private update = () => {
    const position = comparisonPosition(Number(this.range.value));
    this.root.style.setProperty("--compare-position", `${position}%`);
    const before = this.range.dataset.beforeLabel ?? "Before";
    const after = this.range.dataset.afterLabel ?? "After";
    this.range.setAttribute("aria-valuetext", `${position}% ${before}, ${100 - position}% ${after}`);
  };
}

export function comparisonPosition(value: number): number {
  return Number.isFinite(value) ? Math.min(100, Math.max(0, value)) : 50;
}

export type WrapComparison = { width: number; top: number; height: number; repaintWidth: number };

/** A tail | head crop places the periodic join in the middle without generating new media. */
export function wrapComparisonGeometry(width: number, height: number, wrap: WrapComparison) {
  const positive = (value: number) => Number.isSafeInteger(value) && value > 0;
  if (![width, height, wrap.width, wrap.height, wrap.repaintWidth].every(positive)
    || !Number.isSafeInteger(wrap.top) || wrap.top < 0
    || wrap.width > width || wrap.top + wrap.height > height || wrap.repaintWidth > wrap.width) {
    throw new Error("The wrap comparison must fit the source image and contain the repaint band.");
  }
  return {
    imageWidth: width / wrap.width * 100,
    imageHeight: height / wrap.height * 100,
    imageTop: -wrap.top / wrap.height * 100,
    repaintWidth: wrap.repaintWidth / wrap.width * 100,
  };
}
