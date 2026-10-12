import { expect, test } from "bun:test";
import releaseDocument from "../../../content/showcases/manifests/b45587a7593e02d5dc7ca560539b0ab549ea9292078927df837464e9510cc784.json";
import { showcaseLock } from "./catalog";
import { parseRelease } from "./contracts";
import { workflowParallaxGuides } from "./parallax-guides";

test("guide annotations follow the accepted layer results, including the doubled mirror period", () => {
  expect(workflowParallaxGuides()).toEqual([
    { id: "far", file: "layers/far.png", width: 1536, height: 1024, repaintReach: 96 },
    { id: "middle", file: "layers/middle.png", width: 3072, height: 1024 },
    { id: "foreground", file: "layers/foreground.png", width: 1536, height: 1024 },
  ]);
});

test("changed pins or selected media require explicit review of the authored annotations", () => {
  const lock = structuredClone(showcaseLock);
  lock.release.sha256 = "a".repeat(64);
  expect(() => workflowParallaxGuides(lock)).toThrow("Review parallax guides");
  lock.release.sha256 = showcaseLock.release.sha256;
  lock.workflows.parallax.recordings.find(item => item.id === "meadow-01")!.sha256 = "b".repeat(64);
  expect(() => workflowParallaxGuides(lock)).toThrow("Review parallax guides");
  const release = parseRelease(structuredClone(releaseDocument));
  release.files.find(item => item.path === "parallax/layers/far.png")!.sha256 = "c".repeat(64);
  expect(() => workflowParallaxGuides(showcaseLock, release)).toThrow("published far layer");
});
