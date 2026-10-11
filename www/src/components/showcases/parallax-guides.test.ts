import { describe, expect, test } from "bun:test";
import { parallaxGuideLines, validateParallaxGuides, type ParallaxGuideLayer } from "./parallax-guides";

const far: ParallaxGuideLayer = { id: "far", file: "assets/far.png", width: 1536, height: 1024, repaintReach: 96 };
const middle: ParallaxGuideLayer = { id: "middle", file: "assets/middle.png", width: 3072, height: 1024 };
const canvas = { width: 1536, height: 1024 };
const placement = (guide = far) => ({ guide, offset_y: 0, parallax: .25, visible: true });

describe("published parallax guide geometry", () => {
  test("requires exact layer IDs, published files and decoded native dimensions", () => {
    expect([...validateParallaxGuides([far, middle], [middle, far]).keys()]).toEqual(["far", "middle"]);
    expect(validateParallaxGuides([], [far]).size).toBe(0);
    for (const changed of [
      { ...far, id: "another" }, { ...far, file: "assets/another.png" },
      { ...far, width: 3072 }, { ...far, height: 512 },
    ]) expect(() => validateParallaxGuides([changed], [far])).toThrow("do not match");
    expect(() => validateParallaxGuides([far], [far, middle])).toThrow("do not match");
    expect(() => validateParallaxGuides([far, far], [far, middle])).toThrow("do not match");
  });

  test("refuses invalid native sizes or repaint bands", () => {
    for (const repaintReach of [0, -1, 1.5, 769, Infinity]) {
      expect(() => validateParallaxGuides([{ ...far, repaintReach }], [far])).toThrow("Invalid");
    }
    expect(() => validateParallaxGuides([{ ...far, width: 0 }], [{ ...far, width: 0 }])).toThrow("Invalid");
  });

  test("joins track each layer's actual native period and parallax for both scroll directions", () => {
    const layers = [placement(), { ...placement(middle), parallax: .5 }];
    for (const scroll of [0, 400, -400, 1e12, -1e12]) {
      const lines = parallaxGuideLines(layers, scroll, { width: 7000, height: 1024 }, true);
      for (const layer of layers) {
        const joins = lines.filter(line => line.kind === "join" && line.layer === layer.guide.id);
        expect(joins.length).toBeGreaterThan(1);
        for (let index = 0; index < joins.length; index++) {
          const line = joins[index];
          expect(line.x).toBeGreaterThanOrEqual(0);
          expect(line.x).toBeLessThanOrEqual(7000);
          const repeatedIndex = (line.x + scroll * layer.parallax) / layer.guide.width;
          expect(repeatedIndex).toBeCloseTo(Math.round(repeatedIndex), 5);
          if (index) expect(line.x - joins[index - 1].x).toBe(layer.guide.width);
        }
      }
    }
  });

  test("keeps a repaint boundary visible when its repeat join is offscreen", () => {
    // Scroll 200 at .25 moves the nearest join to -50; its right cut is still at 46.
    const lines = parallaxGuideLines([placement()], 200, { width: 1000, height: 1024 }, true);
    expect(lines.filter(line => line.kind === "join")).toEqual([]);
    expect(lines.filter(line => line.kind === "repaint").map(line => line.x)).toEqual([46]);
    const right = parallaxGuideLines([placement()], -4200, { width: 1000, height: 1024 }, true);
    expect(right.filter(line => line.kind === "join")).toEqual([]);
    expect(right.filter(line => line.kind === "repaint").map(line => line.x)).toEqual([954]);
    expect(parallaxGuideLines([placement(middle)], 200, canvas, true).every(line => line.kind === "join")).toBe(true);
  });

  test("follows visibility and toggle state and clips to each layer's vertical extent", () => {
    expect(parallaxGuideLines([placement()], 0, canvas, false)).toEqual([]);
    expect(parallaxGuideLines([{ ...placement(), visible: false }], 0, canvas, true)).toEqual([]);
    expect(parallaxGuideLines([{ ...placement(), offset_y: 1024 }], 0, canvas, true)).toEqual([]);
    for (const [offset_y, top, bottom] of [[-100, 0, 924], [200, 200, 1024]]) {
      const lines = parallaxGuideLines([{ ...placement(), offset_y }], 0, canvas, true);
      expect(lines.length).toBeGreaterThan(0);
      for (const line of lines) expect([line.top, line.bottom]).toEqual([top, bottom]);
    }
  });
});
