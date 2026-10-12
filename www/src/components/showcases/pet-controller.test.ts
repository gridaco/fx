import { describe, expect, test } from "bun:test";
import { parsePetDemo, parseSpriteLayout, type SpriteLayout } from "../../lib/showcases/contracts";
import { SpritePlayer, frameAt, stateLabel } from "./pet-controller";

const layout: SpriteLayout = {
  cell: { width: 192, height: 208 },
  columns: 4,
  rows: [
    { state: "idle", row: 0, frames: 3, durations_ms: [200, 100, 300] },
    { state: "running-left", row: 1, frames: 4, durations_ms: [] },
  ],
};

describe("sprite timing", () => {
  test("holds each frame for its own duration and repeats the row", () => {
    const idle = layout.rows[0];
    expect([0, 199, 200, 299, 300, 599, 600].map(ms => frameAt(idle, ms))).toEqual([0, 0, 1, 1, 2, 2, 0]);
    expect(frameAt(idle, -1)).toBe(2);
  });

  test("a row without timings plays every frame for 140 ms", () => {
    expect([0, 139, 140, 559, 560].map(ms => frameAt(layout.rows[1], ms))).toEqual([0, 0, 1, 3, 0]);
  });
});

describe("sprite player", () => {
  test("advances only while playing, by speed, and reports frame changes", () => {
    const player = new SpritePlayer(layout);
    expect(player.advance(150)).toBe(false);
    player.speed = 2;
    expect(player.advance(30)).toBe(true);
    expect(player.frame).toBe(1);
    player.playing = false;
    expect(player.advance(1000)).toBe(false);
    expect(player.frame).toBe(1);
  });

  test("selecting a state starts it from its first frame", () => {
    const player = new SpritePlayer(layout);
    player.advance(250);
    player.select("running-left");
    expect([player.row.state, player.frame, player.loopMs]).toEqual(["running-left", 0, 560]);
    expect(() => player.select("flying")).toThrow("Unknown sprite state");
  });

  test("stepping pauses and wraps in both directions", () => {
    const player = new SpritePlayer(layout);
    player.step(-1);
    expect([player.playing, player.frame]).toEqual([false, 2]);
    player.step(1);
    expect(player.frame).toBe(0);
    player.step(1);
    expect(player.frame).toBe(1);
  });

  test("a position on the sheet selects its row, clamped to the sheet", () => {
    const player = new SpritePlayer(layout);
    expect([0, .49, .5, .99, 1.2, -.1].map(fraction => player.rowAt(fraction).state)).toEqual(["idle", "idle", "running-left", "running-left", "running-left", "idle"]);
  });

  test("state names read as labels", () => {
    expect(stateLabel("running-right")).toBe("Running right");
  });
});

describe("pet formats", () => {
  test("accepts the layout a pet run writes and refuses inconsistent rows", () => {
    expect(parseSpriteLayout(layout)).toEqual(layout);
    const row = (change: object) => ({ ...layout, rows: [{ ...layout.rows[0], ...change }] });
    expect(() => parseSpriteLayout(row({ row: 1 }))).toThrow("Invalid sprite row");
    expect(() => parseSpriteLayout(row({ frames: 5, durations_ms: [] }))).toThrow("Invalid sprite row");
    expect(() => parseSpriteLayout(row({ durations_ms: [100] }))).toThrow("Invalid sprite row");
    expect(() => parseSpriteLayout({ ...layout, rows: [layout.rows[0], { ...layout.rows[0], row: 1 }] })).toThrow("unique");
  });

  test("the demo names its resources and mirrored states", () => {
    const resources = ["assets/spritesheet.webp", "assets/sprite.json", "assets/base.png", "assets/reference.png"];
    expect(parsePetDemo({ kind: "pet-demo-v1", resources, mirrors: { "running-left": "running-right" } }).mirrors).toEqual({ "running-left": "running-right" });
    expect(() => parsePetDemo({ kind: "pet-demo-v1", resources: resources.slice(1), mirrors: {} })).toThrow("Incomplete demo resources");
    expect(() => parsePetDemo({ kind: "sprite-demo-v1", resources, mirrors: {} })).toThrow("Unsupported showcase format");
  });
});
