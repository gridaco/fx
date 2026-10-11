import { expect, test } from "bun:test";
import { nineSlicePieces, terrainMask, terrainPreset } from "./terrain-ui-controller";

test("the terrain neighbour rule yields exactly the 47 canonical masks", () => {
  const masks = new Set<string>();
  for (let value = 0; value < 512; value++) {
    const rows = Array.from({ length: 3 }, (_, y) => Array.from({ length: 3 }, (_, x) => (value >> (y * 3 + x)) & 1));
    masks.add(terrainMask(rows, 1, 1));
  }
  expect(masks.size).toBe(47);
  expect(terrainMask([[1, 0, 1], [0, 1, 0], [1, 0, 1]], 1, 1)).toBe("000010000");
  expect(terrainMask([[1]], 0, 0)).toBe("000010000");
  for (const name of ["meadow", "islands", "cavern"] as const) {
    const rows = terrainPreset(name);
    expect(rows).toHaveLength(12);
    expect(rows.every(row => row.length === 40)).toBe(true);
    rows.forEach((row, y) => row.forEach((occupied, x) => { if (occupied) expect(masks.has(terrainMask(rows, x, y))).toBe(true); }));
  }
});

test("nine-slice stretches preserve registered corners and cover the destination", () => {
  const cell = { x: 190, y: 210, width: 644, height: 115 };
  const insets = { top: 44, right: 48, bottom: 42, left: 49 };
  const pieces = [...nineSlicePieces(cell, insets, 270, 52, 2, false)];
  expect(pieces).toHaveLength(9);
  expect(pieces[0]).toEqual({ source: { x: 190, y: 210, width: 49, height: 44 }, destination: { x: 0, y: 0, width: 24.5, height: 22 } });
  expect(pieces.reduce((sum, piece) => sum + piece.destination.width * piece.destination.height, 0)).toBeCloseTo(270 * 52);
  expect(pieces[8].destination).toEqual({ x: 246, y: 31, width: 24, height: 21 });
});

test("tiled panel bands crop their final repeat without rescaling the corner", () => {
  const cell = { x: 137, y: 262, width: 751, height: 499 };
  const insets = { top: 99, right: 96, bottom: 96, left: 96 };
  const pieces = [...nineSlicePieces(cell, insets, 740, 440, 2, true)];
  expect(pieces.length).toBeGreaterThan(9);
  expect(pieces[0].destination).toEqual({ x: 0, y: 0, width: 48, height: 49.5 });
  expect(pieces.reduce((sum, piece) => sum + piece.destination.width * piece.destination.height, 0)).toBeCloseTo(740 * 440);
  for (const { source, destination } of pieces) {
    expect(source.x).toBeGreaterThanOrEqual(cell.x);
    expect(source.y).toBeGreaterThanOrEqual(cell.y);
    expect(source.x + source.width).toBeLessThanOrEqual(cell.x + cell.width);
    expect(source.y + source.height).toBeLessThanOrEqual(cell.y + cell.height);
    expect(destination.x + destination.width).toBeLessThanOrEqual(740);
    expect(destination.y + destination.height).toBeLessThanOrEqual(440);
  }
});
