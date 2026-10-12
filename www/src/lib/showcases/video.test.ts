import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { parseDemo, parseVideoDemo, videoDemoResources, type VideoDemoManifest } from "./contracts";
import { showcaseLock } from "./catalog";
import { loadShowcaseJson } from "./load";

/** Authored metadata only: test media is synthesized by the release tests. */
export function videoDemoFixture(): VideoDemoManifest {
  return {
    kind: "video-demo-v1", title: "A short reveal", summary: "A bounded clip from supplied visual inputs.",
    result: { file: "assets/final.mp4", poster: "assets/poster.webp", media_type: "video/mp4", width: 1080, height: 1920, duration_seconds: 4 },
    inputs: [
      { id: "identity", role: "identity", label: "Identity artwork", description: "The supplied character picture.", file: "assets/character.png", media_type: "image/png" },
      { id: "motion", role: "motion", label: "Motion reference", description: "The supplied short performance.", file: "assets/motion.mp4", poster: "assets/motion.webp", media_type: "video/mp4" },
    ],
    stages: [
      { id: "prepare", label: "Prepare", description: "Fit the identity image to the performance.", outputs: [{ file: "assets/character.png", media_type: "image/png", label: "Prepared identity" }] },
      { id: "finish", label: "Finish", description: "Write the final clip and delivery report.", outputs: [{ file: "assets/report.json", media_type: "application/json", label: "Delivery report" }] },
    ],
    limitations: ["A single bounded performance."],
  };
}

test("both video pages use one bounded manifest with complete, deduplicated resources", () => {
  const data = parseVideoDemo(videoDemoFixture());
  expect(data.result).toEqual(videoDemoFixture().result);
  expect(videoDemoResources(data)).toEqual(["assets/final.mp4", "assets/poster.webp", "assets/character.png", "assets/motion.mp4", "assets/motion.webp", "assets/report.json"]);
  for (const slug of ["product-promo", "character-performance"] as const) expect(parseDemo(slug, data)).toEqual(videoDemoResources(data));
});

test("video media paths, actual format declarations and static posters are checked before rendering", () => {
  for (const path of ["../private.mp4", "/tmp/private.mp4", "https://host/private.mp4", "assets/%2e%2e/private.mp4"]) {
    const data = videoDemoFixture(); data.result.file = path;
    expect(() => parseVideoDemo(data)).toThrow("confined");
  }
  const wrongType = videoDemoFixture(); wrongType.result.media_type = "video/webm";
  expect(() => parseVideoDemo(wrongType)).toThrow("filename");
  const animatedPoster = videoDemoFixture(); animatedPoster.result.poster = "assets/preview.mp4";
  expect(() => parseVideoDemo(animatedPoster)).toThrow("static image");
  const missingPoster = videoDemoFixture(); delete missingPoster.inputs[1].poster;
  expect(() => parseVideoDemo(missingPoster)).toThrow("static poster");
  const wrongStageType = videoDemoFixture(); wrongStageType.stages[1].outputs[0].media_type = "image/png";
  expect(() => parseVideoDemo(wrongStageType)).toThrow("filename");
});

test("video dimensions, duration and reference/stage identities cannot be ambiguous", () => {
  for (const [key, value] of [["width", 0], ["height", 1.5], ["duration_seconds", 0], ["duration_seconds", Infinity]] as const) {
    const data = videoDemoFixture(); data.result[key] = value;
    expect(() => parseVideoDemo(data)).toThrow();
  }
  const duplicateInput = videoDemoFixture(); duplicateInput.inputs.push(duplicateInput.inputs[0]);
  expect(() => parseVideoDemo(duplicateInput)).toThrow("unique");
  const duplicateStage = videoDemoFixture(); duplicateStage.stages.push(duplicateStage.stages[0]);
  expect(() => parseVideoDemo(duplicateStage)).toThrow("unique");
  const emptyStage = videoDemoFixture(); emptyStage.stages[0].outputs = [];
  expect(() => parseVideoDemo(emptyStage)).toThrow("recorded outputs");
  const unknown = videoDemoFixture(); (unknown.inputs[0] as { role: string }).role = "unknown";
  expect(() => parseVideoDemo(unknown)).toThrow("role");
});

test("video manifests load from the direct delivery URL and changed bytes fail before rendering", async () => {
  const bytes = Buffer.from(JSON.stringify(videoDemoFixture())), sha256 = createHash("sha256").update(bytes).digest("hex");
  const previous = showcaseLock.workflows["product-promo"], originalFetch = globalThis.fetch;
  showcaseLock.workflows["product-promo"] = { demo: { contract: "video-demo-v1", manifest: { path: "product-promo/manifest.json", sha256, bytes: bytes.length, media_type: "application/json" }, metadata: [] }, recordings: [] };
  try {
    let requested: unknown;
    globalThis.fetch = ((url: unknown) => { requested = url; return Promise.resolve(new Response(bytes)); }) as unknown as typeof fetch;
    const parsed = await loadShowcaseJson("product-promo", "https://assets.example.com/showcases/frozen/product-promo", "manifest.json", parseVideoDemo);
    expect(parsed.result.width).toBe(1080);
    expect(requested).toBe("https://assets.example.com/showcases/frozen/product-promo/manifest.json");
    globalThis.fetch = (() => Promise.resolve(new Response(Buffer.concat([bytes, Buffer.from(" ")])))) as unknown as typeof fetch;
    await expect(loadShowcaseJson("product-promo", "https://assets.example.com/showcases/frozen/product-promo", "manifest.json", parseVideoDemo)).rejects.toThrow("integrity");
  } finally {
    globalThis.fetch = originalFetch;
    if (previous) showcaseLock.workflows["product-promo"] = previous;
    else delete showcaseLock.workflows["product-promo"];
  }
});
