import { expect, test } from "bun:test";
import { parseRunExport } from "@grida/fx-web";
import parallax from "../../../content/showcases/recordings/8610653e0b02cfb004a05163e512e86bca6b8d0368484a2bf413bfb285b461c3.json";
import { showcaseLock } from "./catalog";
import { coverArtifact, workflowCover } from "./covers";

function coverFixture() {
  const document = parseRunExport(structuredClone(parallax));
  const video = { digest: "f".repeat(64), media_type: "video/mp4", name: "preview.mp4", size: 100, path: `artifacts/${"f".repeat(64)}.mp4`, omitted_reason: null };
  document.artifacts.push(video);
  document.view.artifacts.push({ digest: video.digest, kind: video.media_type, name: video.name, size: video.size, url: null, available: true });
  document.view.outputs.preview_video = { file: { digest: video.digest, kind: video.media_type, name: video.name, size: video.size } };
  document.view.outputs.preview_poster = document.view.outputs.preview;
  const lock = structuredClone(showcaseLock);
  const pin = { ...lock.workflows.parallax.recordings[0], id: "meadow-forward-01", fixture: "recordings/cover-fixture.json", sha256: "a".repeat(64), path: `runs/${"a".repeat(64)}/manifest.json` };
  lock.workflows.parallax.recordings = [...lock.workflows.parallax.recordings.filter(item => item.id !== pin.id), pin];
  return { document, lock, pin, documents: { [pin.fixture]: document } };
}

test("video covers resolve named root outputs directly under the selected frozen recording", () => {
  const previous = process.env.FX_SHOWCASE_ASSET_BASE;
  try {
    process.env.FX_SHOWCASE_ASSET_BASE = "https://assets.example.com/media";
    const { lock, pin, documents, document } = coverFixture();
    const cover = workflowCover("parallax", lock, documents);
    expect(cover.kind).toBe("video");
    if (cover.kind !== "video") throw new Error("Expected a recorded video cover.");
    expect(cover.src).toBe(`https://assets.example.com/media/runs/${pin.sha256}/artifacts/${"f".repeat(64)}.mp4`);
    expect(cover.poster).toBe(`https://assets.example.com/media/runs/${pin.sha256}/${coverArtifact(document, "preview_poster", "image").path}`);
    expect(cover.mediaType).toBe("video/mp4");
    document.artifacts.reverse();
    expect(workflowCover("parallax", lock, documents)).toEqual(cover);
    expect(workflowCover("world")).toEqual({ kind: "image", src: `https://assets.example.com/media/showcases/${showcaseLock.release.sha256}/world/cover.webp` });
  } finally {
    if (previous === undefined) delete process.env.FX_SHOWCASE_ASSET_BASE;
    else process.env.FX_SHOWCASE_ASSET_BASE = previous;
  }
});

test("image cover artifacts require an included named image output", () => {
  const { document } = coverFixture();
  document.view.outputs.cover = document.view.outputs.preview;
  const artifact = coverArtifact(document, "cover", "image");
  expect(artifact.media_type).toBe("image/png");
  expect(artifact.path).toBe(`artifacts/${artifact.digest}.png`);
  artifact.path = null;
  artifact.omitted_reason = "policy_excluded";
  expect(() => coverArtifact(document, "cover", "image")).toThrow("Unavailable cover image");
});

test("authored video covers fail visibly when a recording, root output or included media is unavailable", () => {
  const { document, lock, documents } = coverFixture();
  expect(() => workflowCover("parallax", showcaseLock, documents)).toThrow("Missing frozen cover recording");
  expect(() => workflowCover("parallax", lock, {})).toThrow("Missing frozen cover recording");
  expect(() => coverArtifact(document, "missing", "video")).toThrow("cover output missing");
  expect(() => coverArtifact(document, "preview_poster", "video")).toThrow("Unavailable cover video");
  expect(() => coverArtifact(document, "preview_video", "image")).toThrow("Unavailable cover image");
  const artifact = coverArtifact(document, "preview_video", "video");
  artifact.path = null;
  artifact.omitted_reason = "policy_excluded";
  expect(() => coverArtifact(document, "preview_video", "video")).toThrow("Unavailable cover video");
});

test("the published parallax cover selects its recorded local renderer outputs", () => {
  const cover = workflowCover("parallax");
  const pin = showcaseLock.workflows.parallax.recordings.find(item => item.id === "meadow-forward-01")!;
  expect(pin.sha256).toBe("abe44ccedb750bf05633886c052545079f97aee6e83f7161b2611295e4249d3d");
  expect(cover.kind).toBe("video");
  if (cover.kind !== "video") throw new Error("Expected a recorded video cover.");
  expect(cover.src).toEndWith(`/runs/${pin.sha256}/artifacts/ea4210c38afb6657b866ef08614e73b93cb11f6262262830b0fa343794c3c857.mp4`);
  expect(cover.poster).toEndWith(`/runs/${pin.sha256}/artifacts/bfd17c5cbafbcb220da9b45be0e72a9733778feef0759d4cfe446cb56f5c893d.png`);
});

test("the terrain cover uses its free derivative while preserving the original primary run", () => {
  const cover = workflowCover("terrain");
  const pins = showcaseLock.workflows.terrain.recordings;
  const pin = pins.find(item => item.id === "terrain-cover-04")!;
  expect(pins[0]!.id).toBe("terrain-01");
  expect(pin.sha256).toBe("9e7658de4aa6d43d8774a7d5ab76b3ea8c29f883a1acab2be7fdab29765f32ca");
  expect(cover.kind).toBe("video");
  if (cover.kind !== "video") throw new Error("Expected the recorded terrain loop.");
  expect(cover.src).toEndWith(`/runs/${pin.sha256}/artifacts/a4621731541667a062a752405e41814ff0040069ef6f4bbda02ee5eaa73308ff.mp4`);
  expect(cover.poster).toEndWith(`/runs/${pin.sha256}/artifacts/e103b0be751bf77a8a7e0c25925c341773f965aee4092f0f42760fb1d0023b42.webp`);
});

test("the portrait cover selects the recorded native animation with its static fallback and centered cover framing", () => {
  const cover = workflowCover("portrait-motion");
  const pin = showcaseLock.workflows["portrait-motion"].recordings.find(item => item.id === "nami-face-01")!;
  expect(cover.kind).toBe("animated_image");
  if (cover.kind !== "animated_image") throw new Error("Expected the recorded facial animation.");
  expect(cover.src).toEndWith(`/runs/${pin.sha256}/artifacts/593f8553bd6526f4fb75af82c35e2b02371b871dda75fa44124764cd3de0deb4.webp`);
  expect(cover.poster).toEndWith(`/showcases/${showcaseLock.release.sha256}/portrait-motion/cover.webp`);
  expect(cover.fit).toBe("cover");
  expect(cover.position).toBe("center");
});
