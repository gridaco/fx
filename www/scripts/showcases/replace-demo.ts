import { lstat, mkdtemp, rename, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { auditPortableMetadata, demoContracts, parseDemo, parseLock, parseRelease, type DemoRef, type FileRef, type ShowcaseRelease, type ShowcaseSlug } from "../../src/lib/showcases/contracts";
import { confinedFile, contentRoot, deliveryRoot, ensureDirectory, fileRef, files, jsonBytes, plainWrite, sha256, verifiedFile } from "./io";
import { verifyShowcases } from "./verify";

async function existingFile(root: string, path: string): Promise<Buffer | null> {
  const info = await lstat(join(root, path)).catch((error: NodeJS.ErrnoException) => { if (error.code === "ENOENT") return null; throw error; });
  return info ? confinedFile(root, path) : null;
}

/** Replace one reviewed private demo; this stages bytes locally and never publishes them. */
export async function replaceDemo(slug: ShowcaseSlug, source: string, options: { content?: string; delivery?: string } = {}) {
  if (!(slug in demoContracts)) throw new Error(`Unknown showcase ${slug}.`);
  const content = resolve(options.content ?? contentRoot), delivery = resolve(options.delivery ?? deliveryRoot), bundle = resolve(source);
  const lockBytes = await confinedFile(content, "lock.json"), lock = parseLock(JSON.parse(lockBytes.toString()));
  const releaseBytes = await confinedFile(content, lock.release.fixture);
  if (sha256(releaseBytes) !== lock.release.sha256) throw new Error("Frozen release manifest changed.");
  const previous = parseRelease(JSON.parse(releaseBytes.toString()));
  await verifyShowcases({ content });
  const previousRoot = join(delivery, "showcases", lock.release.sha256);
  if (sha256(await confinedFile(previousRoot, "manifest.json")) !== lock.release.sha256) throw new Error("Staged release manifest changed.");

  const manifest = JSON.parse((await confinedFile(bundle, "manifest.json")).toString());
  auditPortableMetadata(manifest);
  const required = ["cover.webp", ...parseDemo(slug, manifest)];
  const paths = await files(bundle);
  for (const path of required) if (!paths.includes(path)) throw new Error(`Missing demo resource ${slug}/${path}.`);
  const replacement: FileRef[] = [];
  for (const path of paths) {
    const bytes = await confinedFile(bundle, path);
    if (path.endsWith(".json")) auditPortableMetadata(JSON.parse(bytes.toString()));
    replacement.push(fileRef(`${slug}/${path}`, bytes));
  }
  const metadata = replacement.filter(file => file.media_type === "application/json");
  const demo: DemoRef = { contract: demoContracts[slug], manifest: metadata.find(file => file.path === `${slug}/manifest.json`)!, metadata: metadata.filter(file => file.path !== `${slug}/manifest.json`) };
  const inventory = [...previous.files.filter(file => !file.path.startsWith(`${slug}/`)), ...replacement].sort((a, b) => a.path.localeCompare(b.path));
  const release: ShowcaseRelease = { kind: "fx-showcase-release-v1", demos: { ...previous.demos, [slug]: demo }, files: inventory };
  parseRelease(release);
  const nextBytes = jsonBytes(release), hash = sha256(nextBytes), relative = `showcases/${hash}`, destination = join(delivery, relative);
  const nextLock = structuredClone(lock);
  nextLock.release = { sha256: hash, path: `${relative}/manifest.json`, fixture: `manifests/${hash}.json` };
  nextLock.workflows[slug].demo = demo;

  await ensureDirectory(join(delivery, "showcases"));
  const staging = await mkdtemp(join(delivery, "showcases/.replace-demo-"));
  const candidate = join(staging, "content"), media = join(staging, "media");
  try {
    for (const file of inventory) {
      const isReplacement = file.path.startsWith(`${slug}/`);
      const bytes = await verifiedFile(isReplacement ? bundle : previousRoot, isReplacement ? { ...file, path: file.path.slice(slug.length + 1) } : file);
      await plainWrite(join(media, file.path), bytes);
      if (file.media_type === "application/json") await plainWrite(join(candidate, "manifests", file.path), bytes);
    }
    await plainWrite(join(media, "manifest.json"), nextBytes);
    await plainWrite(join(candidate, nextLock.release.fixture), nextBytes);
    await plainWrite(join(candidate, "lock.json"), jsonBytes(nextLock));
    for (const item of Object.values(lock.workflows)) for (const recording of item.recordings) {
      await plainWrite(join(candidate, recording.fixture), await confinedFile(content, recording.fixture));
    }
    // This also checks page downloads, embedded references and portrait patch completeness.
    await verifyShowcases({ content: candidate });
    const existing = await lstat(destination).catch((error: NodeJS.ErrnoException) => { if (error.code === "ENOENT") return null; throw error; });
    if (existing) {
      if (sha256(await confinedFile(destination, "manifest.json")) !== hash) throw new Error("Immutable release collision.");
      for (const file of inventory) await verifiedFile(destination, file);
    } else await rename(media, destination);

    if (sha256(await confinedFile(content, "lock.json")) !== sha256(lockBytes)) throw new Error("Content lock changed during demo preparation.");
    const updates = new Map<string, Buffer>([[nextLock.release.fixture, nextBytes]]);
    for (const file of metadata) updates.set(`manifests/${file.path}`, await verifiedFile(destination, file));
    const rollback = new Map<string, Buffer | null>();
    try {
      for (const [path, bytes] of updates) {
        const before = await existingFile(content, path);
        if (before && sha256(before) === sha256(bytes)) continue;
        if (path === nextLock.release.fixture && before) throw new Error("Immutable frozen release collision.");
        rollback.set(path, before);
        await atomicWrite(content, path, bytes);
      }
      if (sha256(await confinedFile(content, "lock.json")) !== sha256(lockBytes)) throw new Error("Content lock changed during demo preparation.");
      await atomicWrite(content, "lock.json", jsonBytes(nextLock));
    } catch (error) {
      for (const [path, bytes] of rollback) {
        if (bytes) await atomicWrite(content, path, bytes);
        else await rm(join(content, path));
      }
      throw error;
    }
  } finally { await rm(staging, { recursive: true, force: true }); }
  return { showcase: slug, release: hash, previous_release: lock.release.sha256, files: inventory.length, delivery_root: delivery, lock: join(content, "lock.json") };
}

async function atomicWrite(root: string, path: string, bytes: Buffer): Promise<void> {
  await ensureDirectory(dirname(join(root, path)));
  await existingFile(root, path);
  const staging = await mkdtemp(join(dirname(join(root, path)), ".demo-metadata-"));
  try {
    await plainWrite(join(staging, "file"), bytes);
    await rename(join(staging, "file"), join(root, path));
  } finally { await rm(staging, { recursive: true, force: true }); }
}

if (import.meta.main) {
  const [slug, source, ...extra] = process.argv.slice(2);
  if (!slug || !(slug in demoContracts) || !source || extra.length) throw new Error("Usage: prepare:demo SHOWCASE COMPLETE_BUNDLE_DIRECTORY");
  console.log(JSON.stringify(await replaceDemo(slug as ShowcaseSlug, source), null, 2));
}
