import { expect, test } from "bun:test";
import { canvasFragment, isWithheldValue, parseCanvasFragment } from "./recorded-canvas-controller";

test("Overview / Canvas fragments retain exact recording, scope and node identities", () => {
  const fragment = canvasFragment("world-01", "scope:world.branch#1", "instance:paint[blue]#2");
  expect(parseCanvasFragment(fragment)).toEqual({ graph: true, recording: "world-01", scope: "scope:world.branch#1", node: "instance:paint[blue]#2" });
  expect(parseCanvasFragment("#graph")).toEqual({ graph: true, recording: null, scope: null, node: null });
  expect(parseCanvasFragment("#usage-title").graph).toBeFalse();
  expect(parseCanvasFragment("").graph).toBeFalse();
});

test("an included author's ordinary JSON object is not mistaken for a disclosure marker", () => {
  expect(isWithheldValue({ withheld: true }, false)).toBeTrue();
  expect(isWithheldValue({ withheld: true }, true)).toBeFalse();
  expect(isWithheldValue({ withheld: false }, false)).toBeFalse();
  expect(isWithheldValue(null, false)).toBeFalse();
});
