/** Authored presentation hints for the exact published layer files, not workflow metadata. */
export type ParallaxGuideLayer = {
  id: string;
  file: string;
  width: number;
  height: number;
  repaintReach?: number;
};

type LoadedLayer = { id: string; file: string; width: number; height: number };
type GuidePlacement = { guide: ParallaxGuideLayer; offset_y: number; parallax: number; visible: boolean };
export type ParallaxGuideLine = { layer: string; kind: "join" | "repaint"; x: number; top: number; bottom: number };

/** Refuse guides for another release or image geometry instead of silently drawing misleading marks. */
export function validateParallaxGuides(guides: readonly ParallaxGuideLayer[], layers: readonly LoadedLayer[]): ReadonlyMap<string, ParallaxGuideLayer> {
  const result = new Map<string, ParallaxGuideLayer>();
  if (!guides.length) return result;
  if (guides.length !== layers.length) throw new Error("Join guides do not match the published layers.");
  for (const guide of guides) {
    const layer = layers.find(item => item.id === guide.id);
    if (result.has(guide.id) || !layer || guide.file !== layer.file || guide.width !== layer.width || guide.height !== layer.height) {
      throw new Error(`Join guides do not match layer ${guide.id}.`);
    }
    if (![guide.width, guide.height].every(value => Number.isSafeInteger(value) && value > 0) ||
      (guide.repaintReach !== undefined && (!Number.isSafeInteger(guide.repaintReach) || guide.repaintReach <= 0 || guide.repaintReach * 2 > guide.width))) {
      throw new Error(`Invalid join guide geometry for layer ${guide.id}.`);
    }
    result.set(guide.id, guide);
  }
  return result;
}

function repeatedPositions(offset: number, period: number, width: number): number[] {
  const phase = ((offset % period) + period) % period;
  const positions: number[] = [];
  for (let x = phase; x <= width; x += period) positions.push(x);
  return positions;
}

/** Repaint boundaries repeat independently: their join may be just outside the viewport. */
export function parallaxGuideLines(layers: readonly GuidePlacement[], scroll: number, canvas: { width: number; height: number }, enabled: boolean): ParallaxGuideLine[] {
  if (!enabled) return [];
  const lines: ParallaxGuideLine[] = [];
  for (const { guide, offset_y, parallax, visible } of layers) {
    if (!visible) continue;
    const top = Math.max(0, offset_y);
    const bottom = Math.min(canvas.height, offset_y + guide.height);
    if (top >= bottom) continue;
    const offset = -scroll * parallax;
    for (const x of repeatedPositions(offset, guide.width, canvas.width)) lines.push({ layer: guide.id, kind: "join", x, top, bottom });
    if (guide.repaintReach !== undefined) {
      for (const side of [-1, 1]) {
        for (const x of repeatedPositions(offset + side * guide.repaintReach, guide.width, canvas.width)) {
          lines.push({ layer: guide.id, kind: "repaint", x, top, bottom });
        }
      }
    }
  }
  return lines;
}
