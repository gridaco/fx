import { expect, test } from "bun:test";
import { parseRunExport } from "@grida/fx-web";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { showcaseLock } from "./catalog";
import { digest, object, parseRelease } from "./contracts";
import { workflowCover } from "./covers";
import { workflowSource } from "./sources";

const content = resolve(import.meta.dir, "../../../content/showcases");
const json = (path: string) => JSON.parse(readFileSync(resolve(content, path), "utf8"));

function recordedDigest(slug: "movie-sprite" | "portrait-motion" | "character-3d", id: string, output: string) {
  const pin = showcaseLock.workflows[slug].recordings.find(item => item.id === id);
  if (!pin) throw new Error(`Missing Nami recording: ${id}`);
  const document = parseRunExport(json(pin.fixture));
  return digest(object(object(document.view.outputs[output], output).file, `${output} file`).digest);
}

test("Nami pages deliver the same reviewed materials as their selected recordings", () => {
  const release = parseRelease(json(showcaseLock.release.fixture));
  const digest = (path: string) => release.files.find(file => file.path === path)?.sha256;
  expect(showcaseLock.workflows["movie-sprite"].recordings[0]?.id).toBe("nami-sprite-03");
  expect(showcaseLock.workflows["portrait-motion"].recordings[0]?.id).toBe("nami-face-01");
  expect(showcaseLock.workflows["character-3d"].recordings[0]?.id).toBe("nami-rig-01");
  // These source identities prevent an old character being relabeled as Nami.
  expect(digest("movie-sprite/assets/source.png")).toBe("90b9cacfc7c3703007dc1f9885dc705fd00643ddc5240358ef87a822f1960a03");
  expect(digest("character-3d/assets/nami-sd.png")).toBe("c5149034bfd086f23a47cf481485c4f9803f043d504431337b549e8b0d840bd6");
  expect(digest("character-3d/assets/model-corrected.glb")).toBe("5c0d91039150a4db1eec6828d88a277cf6599074c0d21a72a504401538f4ed45");
  expect(digest("movie-sprite/assets/canonical.png")).toBe(recordedDigest("movie-sprite", "nami-sprite-03", "canonical"));
  expect(digest("portrait-motion/assets/canonical.png")).toBe(recordedDigest("portrait-motion", "nami-source-03", "portrait"));
  expect(digest("character-3d/assets/model-corrected.glb")).toBe(recordedDigest("character-3d", "nami-rig-01", "character"));
  expect(digest("character-3d/assets/nami-sd.png")).toBe(recordedDigest("character-3d", "nami-rig-01", "references"));
  expect(workflowCover("movie-sprite").kind).not.toBe("image");
  expect(workflowCover("portrait-motion").kind).not.toBe("image");
  expect(workflowCover("character-3d").kind).toBe("video");
  expect(workflowSource("movie-sprite").inputs).toBe("showcase.yaml");
  expect(workflowSource("character-3d").inputs).toBe("inputs/nami/inputs.yaml");
});
