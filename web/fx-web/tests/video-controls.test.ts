import { expect, test } from "bun:test";
import { playerKey, seekTarget, videoTime } from "../src/video-controls";

test("video times read as players show them, and an unknown length as zero", () => {
  expect([0, 7.9, 64, 750, 3723, -3, NaN, Infinity].map(videoTime))
    .toEqual(["0:00", "0:07", "1:04", "12:30", "1:02:03", "0:00", "0:00", "0:00"]);
});

test("seeking stays within the video", () => {
  expect(seekTarget(3, -5, 8)).toBe(0);
  expect(seekTarget(6, 5, 8)).toBe(8);
  expect(seekTarget(2, 5, 8)).toBe(7);
  // A stream of unknown length seeks forward freely.
  expect(seekTarget(2, 5, Infinity)).toBe(7);
});

test("player keys leave the player's own buttons and slider their keys", () => {
  expect(["Escape", " ", "k", "K", "m", "f", "ArrowLeft", "ArrowRight", "x"].map((key) => playerKey(key, "other")))
    .toEqual(["close", "toggle", "toggle", "toggle", "mute", "fullscreen", "back", "forward", null]);
  expect([" ", "Enter", "Escape"].map((key) => playerKey(key, "button"))).toEqual([null, null, "close"]);
  expect(["ArrowLeft", "ArrowRight", " "].map((key) => playerKey(key, "slider"))).toEqual([null, null, "toggle"]);
});
