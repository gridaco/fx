/** Website-owned formats. These are intentionally independent of the public FX export. */
export const demoContracts = {
  parallax: "parallax-demo-v1", world: "world-demo-v1", "movie-sprite": "movie-sprite-demo-v1",
  "portrait-motion": "portrait-motion-demo-v1", "character-3d": "character-3d-demo-v1",
  terrain: "terrain-demo-v1", "ui-kit": "ui-kit-demo-v1", pet: "pet-demo-v1",
  "product-promo": "video-demo-v1", "character-performance": "video-demo-v1",
} as const;
export type ShowcaseSlug = keyof typeof demoContracts;
export const uiGlyphs = ["play", "pause", "close", "menu", "gear", "home", "retry", "check", "search", "hand", "heart", "star", "arrow_left", "arrow_right", "sound_on", "sound_off"] as const;
export const characterRuntimeFiles = ["vendor/model-viewer.min.js", "vendor/meshopt_decoder.module.js", "vendor/meshopt-ready.js", "vendor/draco/draco_decoder.js", "vendor/draco/draco_decoder.wasm", "vendor/draco/draco_wasm_wrapper.js", "vendor/basis/basis_transcoder.js", "vendor/basis/basis_transcoder.wasm"] as const;
export const demoResourceFiles = {
  "movie-sprite": ["assets/loop.webm", "assets/loop.webp", "assets/canonical.png", "assets/original-unpinned-preview.mp4"],
  "portrait-motion": ["assets/face-manifest.json", "assets/canonical.png", "assets/animation.webp"],
  terrain: ["assets/lookup.json", "assets/atlas.png"],
  "ui-kit": ["assets/panel_frame.png", "assets/panel_frame_validation.json", "assets/button_rect.png", "assets/button_rect_validation.json", "assets/preview_icons.png", "assets/preview_icons_validation.json", "assets/meadow.png"],
  pet: ["assets/spritesheet.webp", "assets/sprite.json", "assets/base.png", "assets/reference.png"],
} as const;
export type FileRef = { path: string; sha256: string; bytes: number; media_type: string };
export type RecordingRef = { id: string; label: string; kind: "fx-run-export-v1"; path: string; sha256: string; fixture: string };
export type DemoRef = { contract: string; manifest: FileRef; metadata: FileRef[] };
export type ShowcaseRelease = { kind: "fx-showcase-release-v1"; demos: Record<string, DemoRef>; files: FileRef[] };
export type ShowcaseLock = {
  kind: "fx-showcase-lock-v1";
  release: { sha256: string; path: string; fixture: string };
  workflows: Record<string, { demo: DemoRef; recordings: RecordingRef[] }>;
};

export function object(value: unknown, label = "document"): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`Invalid ${label}: expected an object.`);
  return value as Record<string, unknown>;
}
export function text(value: unknown, label: string): string {
  if (typeof value !== "string" || !value.length) throw new Error(`Invalid ${label}: expected text.`);
  return value;
}
export function number(value: unknown, label: string, minimum = 0): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < minimum) throw new Error(`Invalid ${label}: expected a finite number >= ${minimum}.`);
  return value;
}
export function integer(value: unknown, label: string, minimum = 0): number {
  const result = number(value, label, minimum);
  if (!Number.isSafeInteger(result)) throw new Error(`Invalid ${label}: expected an integer.`);
  return result;
}
export function list(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value)) throw new Error(`Invalid ${label}: expected an array.`);
  return value;
}
export function safePath(value: unknown): string {
  const result = text(value, "resource path");
  if (!/^[a-zA-Z0-9_.-]+(?:\/[a-zA-Z0-9_.-]+)*$/.test(result) || result.split("/").some(part => part === "." || part === "..")) {
    throw new Error("Resource paths must be confined, portable relative paths.");
  }
  return result;
}
export function digest(value: unknown): string {
  const result = text(value, "SHA-256");
  if (!/^[a-f0-9]{64}$/.test(result)) throw new Error("Invalid SHA-256.");
  return result;
}
/** A publication guard for known local coordinates; it cannot certify arbitrary text or media as public-safe. */
export function auditPortableMetadata(value: unknown): void {
  if (typeof value === "string" && (/(?:^|[\s"'])(?:\/(?:Users|home|private|tmp|var\/folders)\/|[A-Za-z]:\\)/.test(value) || /file:\/\/|https?:\/\/[^\s]+[?&](?:X-Amz-|X-Goog-|Signature=|sig=|token=)/i.test(value))) {
    throw new Error("Publication metadata contains a private path or signed resource URL.");
  }
  if (Array.isArray(value)) value.forEach(auditPortableMetadata);
  else if (value && typeof value === "object") for (const [key, item] of Object.entries(value)) {
    if (/^(?:authorization|api_key|access_token|secret_access_key|session_token)$/i.test(key) && item) throw new Error("Publication metadata contains a credential field.");
    auditPortableMetadata(item);
  }
}
function kind(value: Record<string, unknown>, expected: string) {
  if (value.kind !== expected) throw new Error(`Unsupported showcase format: expected ${expected}.`);
}
export function parseFile(value: unknown): FileRef {
  const file = object(value, "file");
  return { path: safePath(file.path), sha256: digest(file.sha256), bytes: integer(file.bytes, "file size"), media_type: text(file.media_type, "media type") };
}
function parseDemoRef(value: unknown): DemoRef {
  const ref = object(value);
  return { contract: text(ref.contract, "adapter contract"), manifest: parseFile(ref.manifest), metadata: list(ref.metadata, "metadata").map(parseFile) };
}
export function parseRelease(value: unknown): ShowcaseRelease {
  const release = object(value); kind(release, "fx-showcase-release-v1");
  const files = list(release.files, "files").map(parseFile);
  if (new Set(files.map(file => file.path)).size !== files.length) throw new Error("Duplicate release file.");
  const demos = Object.fromEntries(Object.entries(object(release.demos)).map(([slug, value]) => {
    if (!(slug in demoContracts)) throw new Error(`Unknown showcase: ${slug}.`);
    const ref = parseDemoRef(value);
    if (ref.contract !== demoContracts[slug as ShowcaseSlug]) throw new Error(`Unsupported ${slug} adapter.`);
    for (const metadata of [ref.manifest, ...ref.metadata]) {
      const file = files.find(file => file.path === metadata.path);
      if (!file || JSON.stringify(file) !== JSON.stringify(metadata)) throw new Error("Demo metadata is outside the release inventory.");
    }
    return [slug, ref];
  }));
  return { kind: "fx-showcase-release-v1", demos, files };
}
export function parseLock(value: unknown): ShowcaseLock {
  const lock = object(value); kind(lock, "fx-showcase-lock-v1");
  const release = object(lock.release);
  const releaseHash = digest(release.sha256);
  if (release.path !== `showcases/${releaseHash}/manifest.json`) throw new Error("Release path must use its manifest digest.");
  const workflows = Object.fromEntries(Object.entries(object(lock.workflows)).map(([slug, value]) => {
    if (!(slug in demoContracts)) throw new Error(`Unknown showcase: ${slug}.`);
    const item = object(value); const demo = parseDemoRef(item.demo);
    if (demo.contract !== demoContracts[slug as ShowcaseSlug]) throw new Error(`Unsupported ${slug} adapter.`);
    const recordings = list(item.recordings, "recordings").map(value => {
      const recording = object(value); const hash = digest(recording.sha256);
      if (recording.kind !== "fx-run-export-v1" || recording.path !== `runs/${hash}/manifest.json`) throw new Error("Unsupported or unpinned recording.");
      return { id: text(recording.id, "recording ID"), label: text(recording.label, "recording label"), kind: "fx-run-export-v1" as const,
        path: safePath(recording.path), sha256: hash, fixture: safePath(recording.fixture) };
    });
    if (new Set(recordings.map(item => item.id)).size !== recordings.length) throw new Error("Duplicate recording ID.");
    return [slug, { demo, recordings }];
  }));
  return { kind: "fx-showcase-lock-v1", release: { sha256: releaseHash, path: safePath(release.path), fixture: safePath(release.fixture) }, workflows };
}

export type ParallaxManifest = { kind: "parallax-demo-v1"; canvas: { width: number; height: number }; layers: { id: string; file: string; offset_y: number; order: number; parallax: number }[] };
export function parseParallax(value: unknown): ParallaxManifest {
  const data = object(value); kind(data, "parallax-demo-v1"); const canvas = object(data.canvas);
  const layers = list(data.layers, "layers").map(value => {
    const item = object(value);
    return { id: text(item.id, "layer ID"), file: safePath(item.file), offset_y: number(item.offset_y, "offset"), order: integer(item.order, "order"), parallax: number(item.parallax, "parallax") };
  });
  if (!layers.length || new Set(layers.map(item => item.id)).size !== layers.length || new Set(layers.map(item => item.order)).size !== layers.length) throw new Error("Layer IDs and order must be unique.");
  return { kind: "parallax-demo-v1", canvas: { width: integer(canvas.width, "canvas width", 1), height: integer(canvas.height, "canvas height", 1) }, layers };
}
export type FaceManifest = { kind: "portrait-patches-v1"; patch_application: string; feather_already_baked: boolean; source_size: [number, number]; patch_size: [number, number]; offset_xy: [number, number]; timeline: { eyes: string; mouth: string; duration_ms: number }[]; patches: { feature_id: string; key: string; state_id: string }[] };
export function parseFace(value: unknown): FaceManifest {
  const data = object(value); kind(data, "portrait-patches-v1");
  if (data.patch_application !== "replace_selected_rgb_preserve_original_alpha" || data.feather_already_baked !== true) throw new Error("Unsupported face patch semantics.");
  const pair = (value: unknown, label: string, minimum = 1): [number, number] => {
    const items = list(value, label); if (items.length !== 2) throw new Error(`Invalid ${label}.`);
    return [integer(items[0], label, minimum), integer(items[1], label, minimum)];
  };
  const patches = list(data.patches, "patches").map(value => {
    const item = object(value); const key = safePath(item.key); if (key.includes("/")) throw new Error("Patch keys cannot contain directories.");
    const feature_id = text(item.feature_id, "feature"); const state_id = text(item.state_id, "state");
    if (!["canvas_left_eye", "canvas_right_eye", "mouth"].includes(feature_id) || !["eyes_half", "eyes_closed", "mouth_a", "mouth_smile"].includes(state_id)) throw new Error("Unknown portrait patch feature or state.");
    return { feature_id, key, state_id };
  });
  const timeline = list(data.timeline, "timeline").map(value => {
    const item = object(value); const eyes = text(item.eyes, "eyes"); const mouth = text(item.mouth, "mouth");
    if (!["rest", "eyes_half", "eyes_closed"].includes(eyes) || !["rest", "mouth_a", "mouth_smile"].includes(mouth)) throw new Error("Unknown timeline expression.");
    return { eyes, mouth, duration_ms: integer(item.duration_ms, "duration", 1) };
  });
  if (patches.length !== 6 || new Set(patches.map(item => item.key)).size !== 6 || !timeline.length) throw new Error("Incomplete portrait patch set.");
  const required = ["canvas_left_eye:eyes_half", "canvas_left_eye:eyes_closed", "canvas_right_eye:eyes_half", "canvas_right_eye:eyes_closed", "mouth:mouth_a", "mouth:mouth_smile"];
  if (!required.every(pair => patches.some(item => `${item.feature_id}:${item.state_id}` === pair))) throw new Error("Incomplete portrait feature states.");
  return { kind: "portrait-patches-v1", patch_application: data.patch_application, feather_already_baked: true, source_size: pair(data.source_size, "source size"), patch_size: pair(data.patch_size, "patch size"), offset_xy: pair(data.offset_xy, "offset", -100000), patches, timeline };
}
export type WorldManifest = { kind: "world-demo-v1"; entities: { entity: { entity_id: string; display_name: string; primary_class: string; entity_kind: string; summary: string; how_it_works_or_lives: string; present_tension: string; facts: { claim: string }[] }; image: string; status: string; concept: { audience_question: string; scene_premise: string }; relationships: { other_entity_id: string; other_display_name: string; summary: string }[]; review?: { verdict: string; what_the_image_teaches: string; blocking_findings: string[]; advisory_findings: string[] } }[] };
export function parseWorld(value: unknown): WorldManifest {
  const data = object(value); kind(data, "world-demo-v1");
  const entities = list(data.entities, "entities").map(value => {
    const item = object(value), entity = object(item.entity), concept = object(item.concept);
    const fields = Object.fromEntries(["entity_id", "display_name", "primary_class", "entity_kind", "summary", "how_it_works_or_lives", "present_tension"].map(key => [key, text(entity[key], key)])) as Omit<WorldManifest["entities"][number]["entity"], "facts">;
    const review = item.review === undefined ? undefined : object(item.review);
    return { entity: { ...fields, facts: list(entity.facts, "facts").map(value => ({ claim: text(object(value).claim, "claim") })) }, image: safePath(item.image), status: text(item.status, "status"),
      concept: { audience_question: text(concept.audience_question, "question"), scene_premise: text(concept.scene_premise, "scene") },
      relationships: list(item.relationships, "relationships").map(value => { const rel = object(value); return { other_entity_id: text(rel.other_entity_id, "entity ID"), other_display_name: text(rel.other_display_name, "display name"), summary: text(rel.summary, "relationship") }; }),
      review: review && { verdict: text(review.verdict, "verdict"), what_the_image_teaches: text(review.what_the_image_teaches, "review"), blocking_findings: list(review.blocking_findings, "findings").map(value => text(value, "finding")), advisory_findings: list(review.advisory_findings, "findings").map(value => text(value, "finding")) } };
  });
  const ids = new Set(entities.map(item => item.entity.entity_id));
  if (!entities.length || ids.size !== entities.length || entities.some(item => item.relationships.some(rel => !ids.has(rel.other_entity_id)))) throw new Error("World entity references are incomplete.");
  return { kind: "world-demo-v1", entities };
}
export type CharacterManifest = { kind: "character-3d-demo-v1"; models: { url: string; stage: string; bytes: number }[]; images: { url: string; label: string; source_label: string }[]; result: { status: string }; runtime_qualified: boolean };
export function parseCharacter(value: unknown): CharacterManifest {
  const data = object(value); kind(data, "character-3d-demo-v1");
  if (typeof data.runtime_qualified !== "boolean") throw new Error("Missing runtime qualification status.");
  const models = list(data.models, "models").map(value => { const item = object(value); return { url: safePath(item.url), stage: text(item.stage, "stage"), bytes: integer(item.bytes, "model size", 1) }; });
  const images = list(data.images, "images").map(value => { const item = object(value); return { url: safePath(item.url), label: text(item.label, "label"), source_label: text(item.source_label, "source label") }; });
  if (!models.length || !images.length || !models.some(item => item.stage === "result")) throw new Error("Incomplete character stages.");
  if (images.filter(item => item.source_label.includes("canonical")).length !== 1) throw new Error("Character requires exactly one canonical reference image.");
  return { kind: "character-3d-demo-v1", models, images, result: { status: text(object(data.result).status, "result status") }, runtime_qualified: data.runtime_qualified };
}
export type SheetLayout = { kind: "ui-sheet-v1"; role: string; canvas: { width: number; height: number }; cells: { cell: { x: number; y: number; width: number; height: number }; state?: string; glyph?: string }[]; insets?: { top: number; right: number; bottom: number; left: number }; draw_scale: number; band_fill?: "tile" | "stretch" };
export function parseSheet(value: unknown): SheetLayout {
  const data = object(value); kind(data, "ui-sheet-v1"); const canvas = object(data.canvas);
  const width = integer(canvas.width, "sheet width", 1), height = integer(canvas.height, "sheet height", 1);
  const role = text(data.role, "sheet role"); if (!["panel_frame", "button_rect", "preview_icons"].includes(role)) throw new Error("Unknown UI sheet role.");
  const cells = list(data.cells, "cells").map(value => { const item = object(value), rect = object(item.cell); const cell = { x: integer(rect.x, "x"), y: integer(rect.y, "y"), width: integer(rect.width, "width", 1), height: integer(rect.height, "height", 1) }; if (cell.x + cell.width > width || cell.y + cell.height > height) throw new Error("UI cell outside sheet."); return { cell, ...(item.state === undefined ? {} : { state: text(item.state, "state") }), ...(item.glyph === undefined ? {} : { glyph: text(item.glyph, "glyph") }) }; });
  if (!cells.length) throw new Error("Empty UI sheet.");
  const insets = data.insets === undefined ? undefined : Object.fromEntries(["top", "right", "bottom", "left"].map(key => [key, integer(object(data.insets)[key], key)])) as SheetLayout["insets"];
  if (role !== "preview_icons" && (!insets || cells.some(({ cell }) => cell.width <= insets.left + insets.right || cell.height <= insets.top + insets.bottom) || !["tile", "stretch"].includes(String(data.band_fill)))) throw new Error("Invalid nine-slice geometry.");
  if (role === "preview_icons" && (cells.length !== uiGlyphs.length || new Set(cells.map(cell => cell.glyph)).size !== uiGlyphs.length || !uiGlyphs.every(glyph => cells.some(cell => cell.glyph === glyph)))) throw new Error("Incomplete icon sheet.");
  if (role === "button_rect" && !["normal", "hover", "pressed", "disabled"].every(state => cells.some(cell => cell.state === state))) throw new Error("Incomplete button states.");
  return { kind: "ui-sheet-v1", role, canvas: { width, height }, cells, insets, draw_scale: number(data.draw_scale, "draw scale", .001), band_fill: data.band_fill as SheetLayout["band_fill"] };
}
export function parseTerrainLookup(value: unknown): { kind: "terrain-lookup-v1"; lookup: Record<string, [number, number]> } {
  const data = object(value); kind(data, "terrain-lookup-v1");
  const lookup = Object.fromEntries(Object.entries(object(data.lookup)).map(([mask, value]) => { const pair = list(value, "atlas coordinate"); if (!/^[01]{9}$/.test(mask) || pair.length !== 2 || integer(pair[0], "column") >= 12 || integer(pair[1], "row") >= 4 || mask[4] !== "1" || [[0, 1, 3], [2, 1, 5], [6, 3, 7], [8, 5, 7]].some(([corner, first, second]) => mask[corner] === "1" && (mask[first] !== "1" || mask[second] !== "1"))) throw new Error("Invalid terrain lookup."); return [mask, pair as [number, number]]; }));
  if (Object.keys(lookup).length !== 47) throw new Error("Terrain requires exactly 47 masks.");
  if (new Set(Object.values(lookup).map(pair => pair.join(","))).size !== 47) throw new Error("Terrain cells must be distinct.");
  return { kind: "terrain-lookup-v1", lookup };
}
export type SpriteLayout = { cell: { width: number; height: number }; columns: number; rows: { state: string; row: number; frames: number; durations_ms: number[] }[] };
/** The sprite.json a pet run writes: equal cells, one row per state, in order. */
export function parseSpriteLayout(value: unknown): SpriteLayout {
  const data = object(value), cell = object(data.cell);
  const width = integer(cell.width, "cell width", 1), height = integer(cell.height, "cell height", 1), columns = integer(data.columns, "columns", 1);
  const rows = list(data.rows, "rows").map((value, index) => {
    const item = object(value), frames = integer(item.frames, "frames", 1);
    const durations_ms = list(item.durations_ms, "durations").map(value => integer(value, "duration", 1));
    if (integer(item.row, "row") !== index || frames > columns || (durations_ms.length && durations_ms.length !== frames)) throw new Error("Invalid sprite row.");
    return { state: text(item.state, "state"), row: index, frames, durations_ms };
  });
  if (!rows.length || new Set(rows.map(item => item.state)).size !== rows.length) throw new Error("Sprite states must be unique.");
  return { cell: { width, height }, columns, rows };
}
export type PetDemo = { kind: "pet-demo-v1"; resources: string[]; mirrors: Record<string, string> };
export function parsePetDemo(value: unknown): PetDemo {
  const data = object(value); kind(data, "pet-demo-v1");
  const resources = list(data.resources, "demo resources").map(safePath);
  if (!demoResourceFiles.pet.every(path => resources.includes(path))) throw new Error("Incomplete demo resources.");
  const mirrors = Object.fromEntries(Object.entries(object(data.mirrors, "mirrors")).map(([state, source]) => [state, text(source, "mirrored state")]));
  return { kind: "pet-demo-v1", resources, mirrors };
}
export const videoDemoSlugs = ["product-promo", "character-performance"] as const;
export type VideoDemoSlug = (typeof videoDemoSlugs)[number];
export type VideoDemoMedia = { file: string; label: string; media_type: string; poster?: string };
export type VideoDemoManifest = {
  kind: "video-demo-v1";
  title: string;
  summary: string;
  result: { file: string; poster: string; media_type: "video/mp4" | "video/webm"; width: number; height: number; duration_seconds: number };
  inputs: (VideoDemoMedia & { id: string; role: "source" | "identity" | "motion" | "direction"; description: string })[];
  stages: { id: string; label: string; description: string; outputs: VideoDemoMedia[] }[];
  limitations: string[];
};
const videoMediaTypes = new Set(["video/mp4", "video/webm"]);
const demoMediaTypes = new Set([...videoMediaTypes, "image/png", "image/jpeg", "image/webp", "image/avif", "application/json", "text/markdown", "text/plain"]);
function videoMediaPath(value: unknown, media_type: string): string {
  const file = safePath(value), extensions: Record<string, string[]> = { "video/mp4": ["mp4"], "video/webm": ["webm"], "image/png": ["png"], "image/jpeg": ["jpg", "jpeg"], "image/webp": ["webp"], "image/avif": ["avif"], "application/json": ["json"], "text/markdown": ["md"], "text/plain": ["txt"] };
  if (!extensions[media_type]?.includes(file.split(".").at(-1)!)) throw new Error("Video demo media type differs from its filename.");
  return file;
}
function videoPoster(value: unknown): string {
  const file = safePath(value);
  if (!/\.(?:png|jpe?g|webp|avif)$/.test(file)) throw new Error("A video poster must be a static image file.");
  return file;
}
function parseVideoMedia(value: unknown): VideoDemoMedia {
  const item = object(value), media_type = text(item.media_type, "media type"), file = videoMediaPath(item.file, media_type);
  if (!demoMediaTypes.has(media_type)) throw new Error("Unsupported video demo media type.");
  const poster = item.poster === undefined ? undefined : videoPoster(item.poster);
  if (videoMediaTypes.has(media_type) && !poster) throw new Error("Video demo clips require a static poster.");
  if (poster === file) throw new Error("A video poster must be a separate image.");
  return { file, media_type, label: text(item.label, "media label"), ...(poster ? { poster } : {}) };
}
/** One bounded result with its actual conditioning inputs and recorded intermediate outputs. */
export function parseVideoDemo(value: unknown): VideoDemoManifest {
  const data = object(value); kind(data, "video-demo-v1");
  const result = object(data.result), media_type = text(result.media_type, "result media type");
  if (!videoMediaTypes.has(media_type)) throw new Error("A video demo result must be an MP4 or WebM clip.");
  const file = videoMediaPath(result.file, media_type), poster = videoPoster(result.poster);
  if (file === poster) throw new Error("A video poster must be a separate image.");
  const inputs = list(data.inputs, "video inputs").map(value => {
    const item = object(value), role = text(item.role, "input role");
    if (!["source", "identity", "motion", "direction"].includes(role)) throw new Error("Unknown video input role.");
    return { ...parseVideoMedia(item), id: text(item.id, "input ID"), role: role as VideoDemoManifest["inputs"][number]["role"], description: text(item.description, "input description") };
  });
  const stages = list(data.stages, "video stages").map(value => {
    const item = object(value), outputs = list(item.outputs, "stage outputs").map(parseVideoMedia);
    if (!outputs.length) throw new Error("A video stage must name its recorded outputs.");
    return { id: text(item.id, "stage ID"), label: text(item.label, "stage label"), description: text(item.description, "stage description"), outputs };
  });
  if (!inputs.length || new Set(inputs.map(item => item.id)).size !== inputs.length) throw new Error("Video input IDs must be present and unique.");
  if (!stages.length || new Set(stages.map(item => item.id)).size !== stages.length) throw new Error("Video stage IDs must be present and unique.");
  return {
    kind: "video-demo-v1", title: text(data.title, "video title"), summary: text(data.summary, "video summary"),
    result: { file, poster, media_type: media_type as VideoDemoManifest["result"]["media_type"], width: integer(result.width, "video width", 1), height: integer(result.height, "video height", 1), duration_seconds: number(result.duration_seconds, "video duration", .001) },
    inputs, stages, limitations: list(data.limitations, "video limitations").map(value => text(value, "limitation")),
  };
}
export function videoDemoResources(data: VideoDemoManifest): string[] {
  return [...new Set([data.result.file, data.result.poster, ...[...data.inputs, ...data.stages.flatMap(item => item.outputs)].flatMap(item => [item.file, ...(item.poster ? [item.poster] : [])])])];
}

/** Validates every format consumed by a bespoke demo, and returns its required relative resources. */
export function parseDemo(slug: ShowcaseSlug, value: unknown): string[] {
  if (slug === "product-promo" || slug === "character-performance") return videoDemoResources(parseVideoDemo(value));
  if (slug === "parallax") return parseParallax(value).layers.map(item => item.file);
  if (slug === "world") return parseWorld(value).entities.map(item => item.image);
  if (slug === "character-3d") { const data = parseCharacter(value); return [...[...data.models, ...data.images].map(item => item.url), ...characterRuntimeFiles]; }
  if (slug === "pet") return parsePetDemo(value).resources;
  const data = object(value); kind(data, demoContracts[slug]);
  const resources = list(data.resources, "demo resources").map(safePath);
  if (!demoResourceFiles[slug].every(path => resources.includes(path))) throw new Error("Incomplete demo resources.");
  return resources;
}
