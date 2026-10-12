import { parseRunExport } from "@grida/fx-web";
import recordingDocument from "../../../content/showcases/recordings/8610653e0b02cfb004a05163e512e86bca6b8d0368484a2bf413bfb285b461c3.json";
import releaseDocument from "../../../content/showcases/manifests/b45587a7593e02d5dc7ca560539b0ab549ea9292078927df837464e9510cc784.json";
import manifestDocument from "../../../content/showcases/manifests/parallax/manifest.json";
import type { ParallaxGuideLayer } from "../../components/showcases/parallax-guides";
import { showcaseLock } from "./catalog";
import { object, parseParallax, parseRelease, text } from "./contracts";

// Refreshed after adding video examples: all five parallax files and its recording are unchanged.
const releaseSha = "b45587a7593e02d5dc7ca560539b0ab549ea9292078927df837464e9510cc784";
const recordingSha = "8610653e0b02cfb004a05163e512e86bca6b8d0368484a2bf413bfb285b461c3";

// Editorial geometry for this reviewed run, not a workflow/export capability.
// Its fixed 192 px repaint band is kept only in the accepted far-layer result.
const layers = [
  { id: "far", width: 1536, height: 1024, repaintReach: 96, source: "parallax.layer['far'].repaint#1", output: "image" },
  { id: "middle", width: 3072, height: 1024, source: "parallax.layer['middle'].mirror#1", output: "image" },
  { id: "foreground", width: 1536, height: 1024, source: "foreground#1", output: "image" },
];

/** Build-time validation prevents old presentation annotations following new content silently. */
export function workflowParallaxGuides(lock = showcaseLock, release = parseRelease(releaseDocument), recording = parseRunExport(recordingDocument)): readonly ParallaxGuideLayer[] {
  const pin = lock.workflows.parallax.recordings.find(item => item.id === "meadow-01");
  if (lock.release.sha256 !== releaseSha || pin?.sha256 !== recordingSha) throw new Error("Review parallax guides for the new showcase content pins.");
  const manifest = parseParallax(manifestDocument);
  const outputDigest = (id: string, output: string) => {
    const node = recording.view.nodes.find(item => item.id === id);
    return text(object(object(node?.outputs[output], "parallax guide output").file, "parallax guide file").digest, "parallax guide digest");
  };
  return layers.map(({ source, output, ...guide }) => {
    const layer = manifest.layers.find(item => item.id === guide.id);
    const chosen = outputDigest(`parallax.layer['${guide.id}'].chosen#1`, "value");
    const file = layer && release.files.find(item => item.path === `parallax/${layer.file}`);
    if (!layer || file?.sha256 !== chosen || chosen !== outputDigest(source, output)) throw new Error(`Parallax guides do not match the published ${guide.id} layer.`);
    return { ...guide, file: layer.file };
  });
}
