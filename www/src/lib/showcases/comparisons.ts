import { parseRunExport, type RunExport } from "@grida/fx-web";
import parallax from "../../../content/showcases/recordings/8610653e0b02cfb004a05163e512e86bca6b8d0368484a2bf413bfb285b461c3.json";
import terrain from "../../../content/showcases/recordings/732df2335d87c9d865351cff997090d0db0b34060cecb8c99757f32201f4ceb5.json";
import uiKit from "../../../content/showcases/recordings/263ad9adb57b9bca9c44cc40e03ae7c93fef3b64cb40f128d46df1e8ea9489b5.json";
import portrait from "../../../content/showcases/recordings/b35ef1984d95307e9bb16741640d5df33d7786a52fa7933d23502072347cabc1.json";
import { deliveryUrl, showcaseLock } from "./catalog";
import { object, text, type ShowcaseSlug } from "./contracts";
import type { WrapComparison } from "../../components/showcases/image-compare-controller";

type ImageSource = { node: string; output: string; key?: string; label: string; alt: string };
type Comparison = {
  recording: string;
  title: string;
  caption: string;
  width: number;
  height: number;
  initial?: number;
  wrap?: WrapComparison;
  before: ImageSource;
  after: ImageSource;
};

// Page-authored pairings name exact recorded steps, rather than guessing from artifact order.
const comparisons: Partial<Record<ShowcaseSlug, readonly Comparison[]>> = {
  parallax: [{
    recording: "meadow-01", title: "Repairing the wrap", width: 1536, height: 1024, initial: 70,
    wrap: { width: 768, top: 256, height: 640, repaintWidth: 192 },
    caption: "The right edge meets the left at the center ticks. Dashed lines bound the repainted band. Drag across the join to compare.",
    before: { node: "far#1", output: "image", label: "Original join", alt: "A close-up of the original layer's right edge meeting its left, with a visible break in the clouds and landscape" },
    after: { node: "parallax.layer['far'].repaint#1", output: "image", label: "Repaired join", alt: "The same repeat join after its accepted edge repaint" },
  }],
  terrain: [{
    recording: "terrain-01", title: "From layout to terrain", width: 2880, height: 960,
    caption: "The fixed 47-mask paint target and the returned image, before local assembly of the final atlas.",
    before: { node: "terrain.paint_target#1", output: "image", label: "Paint target", alt: "The fixed geometry of the 47-mask terrain paint target" },
    after: { node: "terrain.generate#1", output: "image", label: "Generated sheet", alt: "Meadow terrain painted into the same 47-mask layout" },
  }],
  "ui-kit": [{
    recording: "ui-kit-01", title: "The template holds the shape", width: 1024, height: 1024,
    caption: "The guide fixes the four button-state positions. The generated sheet supplies the artwork; local finishing prepares the downloadable sheet.",
    before: { node: "button_rect.template#1", output: "image", label: "Template", alt: "The geometry template for four aligned button states" },
    after: { node: "button_rect.generate#1", output: "image", label: "Generated sheet", alt: "Four meadow-themed button states painted into the template" },
  }],
  "portrait-motion": [{
    recording: "nami-face-01", title: "Small patches, new expressions", width: 2048, height: 2048,
    caption: "The native portrait at rest and with closed eyes and a smile. Pixels outside the facial patches stay unchanged.",
    before: { node: "deliver.render#1", output: "states", key: "rest--rest", label: "Original", alt: "The native portrait with open eyes and a resting mouth" },
    after: { node: "deliver.render#1", output: "states", key: "eyes_closed--mouth_smile", label: "Eyes + mouth", alt: "The same native portrait with closed eyes and a smile composed from patches" },
  }],
};

const documents: Record<string, unknown> = {
  "recordings/8610653e0b02cfb004a05163e512e86bca6b8d0368484a2bf413bfb285b461c3.json": parallax,
  "recordings/732df2335d87c9d865351cff997090d0db0b34060cecb8c99757f32201f4ceb5.json": terrain,
  "recordings/263ad9adb57b9bca9c44cc40e03ae7c93fef3b64cb40f128d46df1e8ea9489b5.json": uiKit,
  "recordings/b35ef1984d95307e9bb16741640d5df33d7786a52fa7933d23502072347cabc1.json": portrait,
};

/** An unavailable or changed editorial selection fails the build instead of silently disappearing. */
export function comparisonArtifact(document: RunExport, source: Pick<ImageSource, "node" | "output" | "key">) {
  const node = document.view.nodes.find(node => node.id === source.node);
  if (!node) throw new Error(`Missing comparison step: ${source.node}.`);
  let value = object(node.outputs[source.output], `comparison output ${source.node}.${source.output}`);
  if (source.key !== undefined) {
    if (!Array.isArray(value.collection)) throw new Error(`Missing comparison collection: ${source.node}.${source.output}.`);
    const entry = value.collection.find((item: unknown) => Array.isArray(item) && item[0] === source.key);
    value = object(entry?.[1], `comparison collection entry ${source.key}`);
  }
  const digest = text(object(value.file, "comparison file").digest, "comparison digest");
  const artifact = document.artifacts.find(artifact => artifact.digest === digest);
  if (!artifact?.path || !artifact.media_type.startsWith("image/")) throw new Error(`Unavailable comparison image: ${source.node}.${source.output}.`);
  return artifact;
}

/** Resolved on the server at build time; the browser receives only pinned image URLs and prose. */
export function workflowComparisons(slug: ShowcaseSlug) {
  return (comparisons[slug] ?? []).map(comparison => {
    const pin = showcaseLock.workflows[slug].recordings.find(recording => recording.id === comparison.recording);
    if (!pin || !documents[pin.fixture]) throw new Error(`Missing frozen comparison recording: ${comparison.recording}.`);
    const document = parseRunExport(documents[pin.fixture]);
    const resolve = (source: ImageSource) => {
      const artifact = comparisonArtifact(document, source);
      return { src: deliveryUrl(`runs/${pin.sha256}/${artifact.path}`), label: source.label, alt: source.alt };
    };
    const before = resolve(comparison.before), after = resolve(comparison.after);
    if (before.src === after.src) throw new Error(`Comparison images must differ: ${comparison.title}.`);
    return { title: comparison.title, caption: comparison.caption, width: comparison.width, height: comparison.height, initial: comparison.initial, wrap: comparison.wrap, before, after };
  });
}
