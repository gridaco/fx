import { parseRunExport } from "@grida/fx-web";
import { lstat, mkdtemp, rename, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { coverArtifact } from "../../src/lib/showcases/covers";
import { auditPortableMetadata, list, object, parseLock, parseVideoDemo, text, videoDemoSlugs, type VideoDemoSlug } from "../../src/lib/showcases/contracts";
import { confinedFile, contentRoot, deliveryRoot, ensureDirectory, files, jsonBytes, mediaType, plainWrite, sha256, wwwRoot } from "./io";
import { appendRecording } from "./recording";
import { replaceDemos } from "./replace-demo";
import { verifyShowcases } from "./verify";

type VideoPreparation = { slug: VideoDemoSlug; bundle: string; recording: { id: string; label: string; source: string } };
export function parseVideoPreparation(value: unknown): VideoPreparation[] {
  const data = object(value);
  if (data.kind !== "fx-video-showcases-preparation-v1") throw new Error("Unsupported video showcase preparation.");
  const entries = list(data.entries, "video preparations").map(value => {
    const item = object(value), slug = text(item.slug, "showcase"), recording = object(item.recording);
    if (!videoDemoSlugs.includes(slug as VideoDemoSlug)) throw new Error("Unknown video showcase.");
    return { slug: slug as VideoDemoSlug, bundle: text(item.bundle, "bundle directory"), recording: { id: text(recording.id, "recording ID"), label: text(recording.label, "recording label"), source: text(recording.source, "export directory") } };
  });
  if (!entries.length || new Set(entries.map(item => item.slug)).size !== entries.length) throw new Error("Video preparations must be present and unique.");
  return entries;
}

/** No semantic acceptance is inferred here: the caller supplies already reviewed exact bytes. */
async function verifyVideoSelection(entry: VideoPreparation) {
  const bundle = resolve(entry.bundle), exported = resolve(entry.recording.source);
  const manifest = JSON.parse((await confinedFile(bundle, "manifest.json")).toString());
  auditPortableMetadata(manifest);
  const demo = parseVideoDemo(manifest), recording = JSON.parse((await confinedFile(exported, "manifest.json")).toString());
  auditPortableMetadata(recording);
  const run = parseRunExport(recording);
  for (const [output, file, kind] of [["video", demo.result.file, "video"], ["poster", demo.result.poster, "image"]] as const) {
    const artifact = coverArtifact(run, output, kind), bytes = await confinedFile(bundle, file);
    if (artifact.media_type !== mediaType(file) || (output === "video" && artifact.media_type !== demo.result.media_type)) throw new Error(`The demo ${output} media type differs from its selected recording output.`);
    if (sha256(bytes) !== artifact.digest || bytes.byteLength !== artifact.size) throw new Error(`The demo ${output} differs from its selected recording output.`);
    const recorded = await confinedFile(exported, artifact.path!);
    if (sha256(recorded) !== artifact.digest || recorded.byteLength !== artifact.size) throw new Error(`The selected recording ${output} bytes changed.`);
  }
  const poster = await confinedFile(bundle, demo.result.poster), cover = await confinedFile(bundle, "cover.webp");
  if (sha256(poster) !== sha256(cover)) throw new Error("The video demo cover must be its exact selected poster.");
}

async function previousFile(root: string, path: string): Promise<Buffer | null> {
  const info = await lstat(join(root, path)).catch((error: NodeJS.ErrnoException) => { if (error.code === "ENOENT") return null; throw error; });
  return info ? confinedFile(root, path) : null;
}
async function atomicWrite(root: string, path: string, bytes: Buffer) {
  await ensureDirectory(dirname(join(root, path)));
  await previousFile(root, path);
  const temporary = await mkdtemp(join(dirname(join(root, path)), ".video-content-"));
  try {
    await plainWrite(join(temporary, "file"), bytes);
    await rename(join(temporary, "file"), join(root, path));
  } finally { await rm(temporary, { recursive: true, force: true }); }
}

/** Stages immutable bytes first; commits both demo/recording selections through one final lock update. */
export async function prepareVideoShowcases(entries: readonly VideoPreparation[], options: { content?: string; delivery?: string; scratch?: string } = {}) {
  parseVideoPreparation({ kind: "fx-video-showcases-preparation-v1", entries });
  for (const entry of entries) await verifyVideoSelection(entry);
  const content = resolve(options.content ?? contentRoot), delivery = resolve(options.delivery ?? deliveryRoot), scratch = resolve(options.scratch ?? join(wwwRoot, ".showcases"));
  const before = await confinedFile(content, "lock.json");
  await ensureDirectory(scratch);
  const staging = await mkdtemp(join(scratch, "video-preparation-")), candidate = join(staging, "content");
  try {
    for (const path of await files(content)) await plainWrite(join(candidate, path), await confinedFile(content, path));
    const release = await replaceDemos(entries.map(entry => ({ slug: entry.slug, source: entry.bundle })), { content: candidate, delivery });
    const recordings = [];
    for (const entry of entries) recordings.push(await appendRecording(entry.slug, entry.recording.id, entry.recording.label, entry.recording.source, { content: candidate, delivery, primary: true }));
    await verifyShowcases({ content: candidate, requireRecordings: true });
    const lock = parseLock(JSON.parse((await confinedFile(candidate, "lock.json")).toString()));
    const pins = videoDemoSlugs.flatMap(slug => lock.workflows[slug]?.recordings[0] ? [lock.workflows[slug].recordings[0]] : []);
    const imports = [...new Map(pins.map(pin => [pin.fixture, pin])).values()];
    const index = ["/** Generated by prepare:video from selected immutable FX export manifests. */", ...imports.map((pin, index) => `import recording${index} from ${JSON.stringify(`./${pin.fixture}`)};`), "", "export const videoRecordings: Record<string, unknown> = {", ...imports.map((pin, index) => `  ${JSON.stringify(pin.fixture)}: recording${index},`), "};", ""].join("\n");
    await plainWrite(join(candidate, "video-recordings.ts"), Buffer.from(index));
    if (sha256(await confinedFile(content, "lock.json")) !== sha256(before)) throw new Error("Content lock changed during video preparation.");
    const rollback = new Map<string, Buffer | null>();
    try {
      for (const path of (await files(candidate)).filter(path => path !== "lock.json")) {
        const next = await confinedFile(candidate, path), previous = await previousFile(content, path);
        if (previous && sha256(previous) === sha256(next)) continue;
        if (previous && /^recordings\/[a-f0-9]{64}\.json$|^manifests\/[a-f0-9]{64}\.json$/.test(path)) throw new Error("Immutable frozen content collision.");
        rollback.set(path, previous);
        await atomicWrite(content, path, next);
      }
      if (sha256(await confinedFile(content, "lock.json")) !== sha256(before)) throw new Error("Content lock changed during video preparation.");
      await atomicWrite(content, "lock.json", await confinedFile(candidate, "lock.json"));
    } catch (error) {
      for (const [path, previous] of rollback) {
        if (previous) await atomicWrite(content, path, previous);
        else await rm(join(content, path));
      }
      throw error;
    }
    return { release: release.release, previous_release: release.previous_release, recordings, lock: join(content, "lock.json") };
  } finally { await rm(staging, { recursive: true, force: true }); }
}

if (import.meta.main) {
  const [path, ...extra] = process.argv.slice(2);
  if (!path || extra.length) throw new Error("Usage: prepare:video LOCAL_PREPARATION_JSON");
  const source = resolve(path), config = JSON.parse((await confinedFile(dirname(source), source.split("/").at(-1)!)).toString());
  console.log(JSON.stringify(await prepareVideoShowcases(parseVideoPreparation(config)), null, 2));
}
