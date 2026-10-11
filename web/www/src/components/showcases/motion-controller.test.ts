import { describe, expect, test } from "bun:test";
import { replacePatchRgb, selectExpression } from "./motion-controller";

describe("native face composition", () => {
  test("replaces selected RGB without changing alpha or neighboring pixels", () => {
    const frame = new Uint8ClampedArray([10, 20, 30, 128, 40, 50, 60, 0, 70, 80, 90, 255]);
    replacePatchRgb(frame, [[0, 100, 110, 120], [8, 130, 140, 150]]);
    expect([...frame]).toEqual([100, 110, 120, 128, 40, 50, 60, 0, 130, 140, 150, 255]);
  });

  test("uses exact segment boundaries and repeats the authored timeline", () => {
    const timeline = [{ eyes: "rest", mouth: "rest", duration_ms: 1000 }, { eyes: "closed", mouth: "smile", duration_ms: 120 }];
    expect(selectExpression(timeline, .999).eyes).toBe("rest");
    expect(selectExpression(timeline, 1)).toMatchObject({ eyes: "closed", mouth: "smile" });
    expect(selectExpression(timeline, 1.12).eyes).toBe("rest");
    expect(selectExpression(timeline, -.1).eyes).toBe("closed");
  });
});
