import { parseRunExport, type RunExport } from "@grida/fx-web";
import parallaxPreview from "../../../content/showcases/recordings/abe44ccedb750bf05633886c052545079f97aee6e83f7161b2611295e4249d3d.json";
import terrainCover from "../../../content/showcases/recordings/9e7658de4aa6d43d8774a7d5ab76b3ea8c29f883a1acab2be7fdab29765f32ca.json";
import portraitAnimation from "../../../content/showcases/recordings/b35ef1984d95307e9bb16741640d5df33d7786a52fa7933d23502072347cabc1.json";
import namiRigCover from "../../../content/showcases/recordings/ebf0befefd7d408fe27c07c942850e9d4c036158a6472a9a26f93824531b6aab.json";
import namiSprite from "../../../content/showcases/recordings/c1ac112e0ea3c2800db925dafe671dda01538f45bc981235b203a0b73bb251c0.json";
import coralReel from "../../../content/showcases/recordings/2d2d37fc6940c7063dac776e6968ace48e83eff51ff7b7a9d8640599aaaf5c96.json";
import { deliveryUrl, showcaseLock, workflowAssetPath } from "./catalog";
import { object, text, type ShowcaseLock, type ShowcaseSlug } from "./contracts";

type CoverFraming = { fit?: "cover" | "contain"; position?: string };
export type WorkflowCoverMedia = CoverFraming & (
  | { kind: "image"; src: string }
  | { kind: "animated_image"; src: string; poster: string }
  | { kind: "video"; src: string; poster: string; mediaType: "video/mp4" | "video/webm" }
);

// Covers select workflow outputs explicitly; the shared card never interprets a workflow's internals.
type OutputCover = { recording: string } & CoverFraming & (
  | { kind: "video"; video: string; poster: string }
  | { kind: "animated_image"; image: string }
  | { kind: "image"; image: string }
);
const outputCovers: Partial<Record<ShowcaseSlug, OutputCover>> = {
  parallax: { kind: "video", recording: "meadow-forward-01", video: "preview_video", poster: "preview_poster" },
  terrain: { kind: "video", recording: "terrain-cover-04", video: "cover_video", poster: "cover" },
  "movie-sprite": { kind: "animated_image", recording: "nami-sprite-03", image: "cover", fit: "cover", position: "center" },
  "character-3d": { kind: "video", recording: "nami-rig-cover-02", video: "cover_video", poster: "cover", fit: "cover", position: "center" },
  "portrait-motion": { kind: "animated_image", recording: "nami-face-01", image: "animation", fit: "cover", position: "center" },
  pet: { kind: "video", recording: "coral-reel-01", video: "video", poster: "poster" },
};

const documents: Record<string, unknown> = {
  "recordings/c1ac112e0ea3c2800db925dafe671dda01538f45bc981235b203a0b73bb251c0.json": namiSprite,
  "recordings/ebf0befefd7d408fe27c07c942850e9d4c036158a6472a9a26f93824531b6aab.json": namiRigCover,
  "recordings/abe44ccedb750bf05633886c052545079f97aee6e83f7161b2611295e4249d3d.json": parallaxPreview,
  "recordings/9e7658de4aa6d43d8774a7d5ab76b3ea8c29f883a1acab2be7fdab29765f32ca.json": terrainCover,
  "recordings/b35ef1984d95307e9bb16741640d5df33d7786a52fa7933d23502072347cabc1.json": portraitAnimation,
  "recordings/2d2d37fc6940c7063dac776e6968ace48e83eff51ff7b7a9d8640599aaaf5c96.json": coralReel,
};
const videoTypes = new Set(["video/mp4", "video/webm"]);
const imageTypes = new Set(["image/png", "image/jpeg", "image/webp", "image/avif"]);

/** Only an included file from a named root output can become published cover media. */
export function coverArtifact(document: RunExport, output: string, media: "video" | "image") {
  const value = object(document.view.outputs[output], `cover output ${output}`);
  const hash = text(object(value.file, `cover output file ${output}`).digest, "cover digest");
  const artifact = document.artifacts.find(item => item.digest === hash);
  if (!artifact?.path || !(media === "video" ? videoTypes : imageTypes).has(artifact.media_type)) {
    throw new Error(`Unavailable cover ${media}: ${output}.`);
  }
  return artifact;
}

/** Build-time resolution keeps frozen run documents out of the browser bundle. */
export function workflowCover(slug: ShowcaseSlug, lock: ShowcaseLock = showcaseLock, recordings: Readonly<Record<string, unknown>> = documents): WorkflowCoverMedia {
  const selection = outputCovers[slug];
  if (!selection) return { kind: "image", src: workflowAssetPath(slug, "cover.webp") };
  const pin = lock.workflows[slug]?.recordings.find(item => item.id === selection.recording);
  if (!pin || !recordings[pin.fixture]) throw new Error(`Missing frozen cover recording: ${selection.recording}.`);
  const document = parseRunExport(recordings[pin.fixture]);
  if (selection.kind === "image" || selection.kind === "animated_image") {
    const artifact = coverArtifact(document, selection.image, "image");
    const framing: CoverFraming = { fit: selection.fit, position: selection.position };
    const src = deliveryUrl(`runs/${pin.sha256}/${artifact.path}`);
    return selection.kind === "animated_image"
      ? { kind: "animated_image", src, poster: workflowAssetPath(slug, "cover.webp"), ...framing }
      : { kind: "image", src, ...framing };
  }
  const video = coverArtifact(document, selection.video, "video"), poster = coverArtifact(document, selection.poster, "image");
  return {
    kind: "video",
    src: deliveryUrl(`runs/${pin.sha256}/${video.path}`),
    poster: deliveryUrl(`runs/${pin.sha256}/${poster.path}`),
    mediaType: video.media_type as "video/mp4" | "video/webm",
    fit: selection.fit,
    position: selection.position,
  };
}

/**
 * The run the landing reel shows behind a result: the workflow's first recording, the one its
 * Workflow tab opens on. A cover can come from a smaller run (a reel or cover render), so the
 * canvas starts on the card holding the cover's own image when that run has it.
 */
export function coverRecording(slug: ShowcaseSlug, lock: ShowcaseLock = showcaseLock, recordings: Readonly<Record<string, unknown>> = documents): { url: string; sha256: string; focus: string | null } | null {
  const pins = lock.workflows[slug]?.recordings ?? [];
  const pin = pins[0];
  if (!pin) return null;
  const selection = outputCovers[slug];
  const coverPin = selection && pins.find(item => item.id === selection.recording);
  let focus: string | null = null;
  if (selection && coverPin && recordings[coverPin.fixture]) {
    const document = parseRunExport(recordings[coverPin.fixture]);
    focus = coverArtifact(document, selection.kind === "video" ? selection.poster : selection.image, "image").digest;
  }
  return { url: deliveryUrl(pin.path), sha256: pin.sha256, focus };
}
