import { expect, test } from "bun:test";
import { wrapComparisonGeometry } from "./image-compare-controller";

test("wrap close-up puts the last and first source columns together at the center", () => {
  const view = { width: 768, top: 256, height: 640, repaintWidth: 192 };
  const geometry = wrapComparisonGeometry(1536, 1024, view);
  // CSS places the tail at 50% minus the image width, and the head at 50%.
  const tailEnd = (50 - geometry.imageWidth) / 100 * view.width + geometry.imageWidth / 100 * view.width;
  const headStart = .5 * view.width;
  expect(tailEnd).toBe(view.width / 2);
  expect(headStart).toBe(tailEnd);
  expect(geometry.imageWidth / 100 * view.width).toBe(1536);
  expect(geometry.imageHeight / 100 * view.height).toBe(1024);
  expect(geometry.imageTop / 100 * view.height).toBe(-view.top);
  expect(geometry.repaintWidth / 100 * view.width).toBe(view.repaintWidth);
});

test("wrap views refuse crops outside the image or a band wider than their context", () => {
  const view = { width: 768, top: 256, height: 640, repaintWidth: 192 };
  for (const change of [{ width: 1600 }, { top: -1 }, { top: 600 }, { height: 0 }, { repaintWidth: 900 }]) {
    expect(() => wrapComparisonGeometry(1536, 1024, { ...view, ...change })).toThrow("must fit");
  }
});
