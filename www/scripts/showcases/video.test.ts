import { expect, test } from "bun:test";
import { parseRunExport } from "@grida/fx-web";
import { cp, mkdtemp, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import originalRun from "../../content/showcases/recordings/8610653e0b02cfb004a05163e512e86bca6b8d0368484a2bf413bfb285b461c3.json";
import { parseLock, parseRelease, videoDemoSlugs, type VideoDemoSlug } from "../../src/lib/showcases/contracts";
import { confinedFile, contentRoot, fileRef, jsonBytes, plainWrite, sha256, verifiedFile } from "./io";
import { parseVideoPreparation, prepareVideoShowcases } from "./video";
import { verifyShowcases } from "./verify";

/** Codec-free authored bytes test packaging identities without generated art or provider calls. */
async function fixture(root: string) {
  const content = join(root, "content"), delivery = join(root, "delivery"), scratch = join(root, "scratch");
  await cp(contentRoot, content, { recursive: true });
  const lock = parseLock(JSON.parse((await confinedFile(content, "lock.json")).toString())), original = parseRelease(JSON.parse((await confinedFile(content, lock.release.fixture)).toString()));
  const existing = original.files.filter(file => !videoDemoSlugs.some(slug => file.path.startsWith(`${slug}/`))), data = new Map<string, Buffer>();
  for (const file of existing) data.set(file.path, file.media_type === "application/json" ? await verifiedFile(join(content, "manifests"), file) : Buffer.from(`synthetic:${file.path}`));
  const inventory = existing.map(file => fileRef(file.path, data.get(file.path)!)), refs = new Map(inventory.map(file => [file.path, file]));
  const demos = Object.fromEntries(Object.entries(original.demos).filter(([slug]) => !videoDemoSlugs.includes(slug as VideoDemoSlug)).map(([slug, demo]) => [slug, { ...demo, manifest: refs.get(demo.manifest.path)!, metadata: demo.metadata.map(file => refs.get(file.path)!) }]));
  const release = { ...original, demos, files: inventory }, releaseBytes = jsonBytes(release), hash = sha256(releaseBytes), previousRoot = join(delivery, "showcases", hash);
  for (const [path, bytes] of data) await plainWrite(join(previousRoot, path), bytes);
  await plainWrite(join(previousRoot, "manifest.json"), releaseBytes);
  await plainWrite(join(content, "manifests", `${hash}.json`), releaseBytes);
  lock.release = { sha256: hash, path: `showcases/${hash}/manifest.json`, fixture: `manifests/${hash}.json` };
  for (const slug of videoDemoSlugs) delete lock.workflows[slug];
  for (const [slug, demo] of Object.entries(demos)) lock.workflows[slug].demo = demo;
  await plainWrite(join(content, "lock.json"), jsonBytes(lock));

  const entries = [];
  for (const slug of videoDemoSlugs) {
    const bundle = join(root, slug, "bundle"), source = join(root, slug, "export"), video = Buffer.from(`synthetic-video:${slug}`), poster = Buffer.from(`synthetic-poster:${slug}`);
    const manifest = {
      kind: "video-demo-v1", title: "A bounded clip", summary: "One result from supplied visual inputs.",
      result: { file: "assets/final.mp4", poster: "assets/poster.webp", media_type: "video/mp4", width: 640, height: 360, duration_seconds: 4 },
      inputs: slug === "product-promo"
        ? [{ id: "source", role: "source", label: "Source input", description: "The supplied input.", file: "assets/source.mp4", media_type: "video/mp4", poster: "assets/source.webp" }]
        : [{ id: "first", role: "identity", label: "First identity", description: "The first supplied identity.", file: "assets/character.png", media_type: "image/png" }, { id: "second", role: "identity", label: "Second identity", description: "The second supplied identity.", file: "assets/nami.png", media_type: "image/png" }],
      stages: [{ id: "finish", label: "Finish", description: "Write the selected outputs.", outputs: [{ file: "assets/report.json", label: "Report", media_type: "application/json" }] }], limitations: ["A bounded demonstration."],
    };
    await plainWrite(join(bundle, "manifest.json"), jsonBytes(manifest));
    await plainWrite(join(bundle, "assets/final.mp4"), video);
    await plainWrite(join(bundle, "assets/poster.webp"), poster);
    await plainWrite(join(bundle, "cover.webp"), poster);
    await plainWrite(join(bundle, "assets/report.json"), jsonBytes({ kind: "synthetic-report", seconds: 4 }));
    if (slug === "product-promo") await plainWrite(join(bundle, "assets/edit.json"), jsonBytes({ kind: "synthetic-edit", seconds: 4 }));
    for (const input of manifest.inputs) await plainWrite(join(bundle, input.file), Buffer.from(`synthetic-input:${slug}:${input.id}`));
    if (slug === "product-promo") await plainWrite(join(bundle, "assets/source.webp"), Buffer.from("synthetic-source-poster"));
    const run = parseRunExport(structuredClone(originalRun));
    // Original artifacts are explicitly unavailable; only this test's new bytes are included.
    for (const artifact of run.artifacts) { artifact.path = null; artifact.omitted_reason = "policy_excluded"; }
    for (const artifact of run.view.artifacts) artifact.available = false;
    for (const [name, bytes, extension, media_type] of [["video", video, "mp4", "video/mp4"], ["poster", poster, "webp", "image/webp"]] as const) {
      const digest = sha256(bytes), artifact = { digest, media_type, name, size: bytes.length, path: `artifacts/${digest}.${extension}`, omitted_reason: null };
      run.artifacts.push(artifact);
      run.view.artifacts.push({ digest, kind: media_type, name, size: bytes.length, url: null, available: true });
      run.view.outputs[name] = { file: { digest, kind: media_type, name, size: bytes.length } };
      await plainWrite(join(source, artifact.path), bytes);
    }
    parseRunExport(run);
    await plainWrite(join(source, "manifest.json"), jsonBytes(run));
    entries.push({ slug, bundle, recording: { id: `${slug}-01`, label: "Selected clip", source } });
  }
  return { content, delivery, scratch, entries, lock, previousRoot };
}

test("video preparation commits both real selections together and preserves all existing demos and recordings", async () => {
  const root = await mkdtemp(join(await realpath(tmpdir()), "fx-video-showcases-"));
  try {
    const item = await fixture(root), result = await prepareVideoShowcases(item.entries, item);
    const after = parseLock(JSON.parse((await confinedFile(item.content, "lock.json")).toString()));
    for (const [slug, workflow] of Object.entries(item.lock.workflows)) expect(after.workflows[slug]).toEqual(workflow);
    for (const entry of item.entries) expect(after.workflows[entry.slug].recordings[0].id).toBe(entry.recording.id);
    expect(result.previous_release).toBe(item.lock.release.sha256);
    expect(result.release).not.toBe(item.lock.release.sha256);
    await expect(verifyShowcases({ content: item.content, requireRecordings: true })).resolves.toHaveProperty("release", result.release);
    const index = (await confinedFile(item.content, "video-recordings.ts")).toString();
    for (const recording of result.recordings) expect(index).toContain(`recordings/${recording.sha256}.json`);
    const before = await confinedFile(item.content, "lock.json");
    expect((await prepareVideoShowcases(item.entries, item)).release).toBe(result.release);
    expect(await confinedFile(item.content, "lock.json")).toEqual(before);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("a mismatched final, poster or recording fails before any content lock update", async () => {
  for (const changed of ["video", "poster", "recording", "cover", "video-format", "poster-format"] as const) {
    const root = await mkdtemp(join(await realpath(tmpdir()), "fx-video-mismatch-"));
    try {
      const item = await fixture(root), before = await confinedFile(item.content, "lock.json"), entry = item.entries[1];
      if (changed === "video") await plainWrite(join(entry.bundle, "assets/final.mp4"), Buffer.from("different-video"));
      if (changed === "poster") await plainWrite(join(entry.bundle, "assets/poster.webp"), Buffer.from("different-poster"));
      if (changed === "cover") await plainWrite(join(entry.bundle, "cover.webp"), Buffer.from("different-cover"));
      if (changed === "recording") {
        const run = parseRunExport(JSON.parse((await confinedFile(entry.recording.source, "manifest.json")).toString())), video = run.artifacts.find(item => item.name === "video")!;
        await plainWrite(join(entry.recording.source, video.path!), Buffer.from("different-recording-video"));
      }
      if (changed === "video-format" || changed === "poster-format") {
        const run = parseRunExport(JSON.parse((await confinedFile(entry.recording.source, "manifest.json")).toString())), name = changed === "video-format" ? "video" : "poster", artifact = run.artifacts.find(item => item.name === name)!;
        const bytes = await confinedFile(entry.recording.source, artifact.path!);
        artifact.media_type = name === "video" ? "video/webm" : "image/jpeg";
        artifact.path = `artifacts/${artifact.digest}.${name === "video" ? "webm" : "jpg"}`;
        await plainWrite(join(entry.recording.source, artifact.path), bytes);
        run.view.artifacts.find(item => item.digest === artifact.digest)!.kind = artifact.media_type;
        (run.view.outputs[name] as { file: { kind: string } }).file.kind = artifact.media_type;
        await plainWrite(join(entry.recording.source, "manifest.json"), jsonBytes(run));
      }
      await expect(prepareVideoShowcases(item.entries, item)).rejects.toThrow(changed === "cover" ? "exact selected poster" : changed === "recording" ? "bytes changed" : "differs");
      expect(await confinedFile(item.content, "lock.json")).toEqual(before);
    } finally { await rm(root, { recursive: true, force: true }); }
  }
});

test("private preparation configuration is explicit and does not enter the portable manifest", () => {
  expect(() => parseVideoPreparation({ kind: "unknown", entries: [] })).toThrow("Unsupported");
  expect(() => parseVideoPreparation({ kind: "fx-video-showcases-preparation-v1", entries: [] })).toThrow("present");
  const item = { slug: "product-promo" as const, bundle: "/tmp/local-bundle", recording: { id: "promo-01", label: "Selected clip", source: "/tmp/local-export" } };
  expect(parseVideoPreparation({ kind: "fx-video-showcases-preparation-v1", entries: [item] })).toEqual([item]);
  expect(() => parseVideoPreparation({ kind: "fx-video-showcases-preparation-v1", entries: [item, item] })).toThrow("unique");
});
