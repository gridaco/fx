import { lstat, mkdtemp, rename, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { auditPortableMetadata, demoContracts, parseDemo, parseLock, parseRelease, type DemoRef, type FileRef, type ShowcaseRelease, type ShowcaseSlug } from "../../src/lib/showcases/contracts";
import { confinedFile, contentRoot, deliveryRoot, ensureDirectory, fileRef, files, jsonBytes, plainWrite, sha256, verifiedFile } from "./io";
import { verifyShowcases } from "./verify";

async function existingFile(root: string, path: string): Promise<Buffer | null> {
  const info = await lstat(join(root, path)).catch((error: NodeJS.ErrnoException) => { if (error.code === "ENOENT") return null; throw error; });
  return info ? confinedFile(root, path) : null;
}

/** Add or replace one reviewed private demo; this stages bytes locally and never publishes them. */
export async function replaceDemo(slug: ShowcaseSlug, source: string, options: { content?: string; delivery?: string } = {}) {
  const result = await replaceDemos([{ slug, source }], options);
  return { showcase: slug, added: result.showcases[0].added, release: result.release, previous_release: result.previous_release, files: result.files, delivery_root: result.delivery_root, lock: result.lock };
}

/** Stage a complete set of reviewed demo projections under one atomic content-lock update. */
export async function replaceDemos(items: readonly { slug: ShowcaseSlug; source: string }[], options: { content?: string; delivery?: string } = {}) {
  if (!items.length || new Set(items.map(item => item.slug)).size !== items.length) throw new Error("Demo preparation needs unique showcases.");
  for (const item of items) if (!(item.slug in demoContracts)) throw new Error(`Unknown showcase ${item.slug}.`);
  const content = resolve(options.content ?? contentRoot), delivery = resolve(options.delivery ?? deliveryRoot);
  const lockBytes = await confinedFile(content, "lock.json"), lock = parseLock(JSON.parse(lockBytes.toString()));
  const releaseBytes = await confinedFile(content, lock.release.fixture);
  if (sha256(releaseBytes) !== lock.release.sha256) throw new Error("Frozen release manifest changed.");
  const previous = parseRelease(JSON.parse(releaseBytes.toString()));
  const additions = items.filter(item => !(item.slug in lock.workflows)).map(item => item.slug);
  await verifyShowcases({ content, addedDemos: additions, previousDemos: items.filter(item => !additions.includes(item.slug)).map(item => item.slug) });
  const previousRoot = join(delivery, "showcases", lock.release.sha256);
  if (sha256(await confinedFile(previousRoot, "manifest.json")) !== lock.release.sha256) throw new Error("Staged release manifest changed.");

  const replacement: FileRef[] = [];
  const demos = new Map<ShowcaseSlug, DemoRef>();
  for (const { slug, source } of items) {
    const bundle = resolve(source), manifest = JSON.parse((await confinedFile(bundle, "manifest.json")).toString());
    auditPortableMetadata(manifest);
    const required = ["cover.webp", ...parseDemo(slug, manifest)], paths = await files(bundle), references: FileRef[] = [];
    for (const path of required) if (!paths.includes(path)) throw new Error(`Missing demo resource ${slug}/${path}.`);
    for (const path of paths) {
      const bytes = await confinedFile(bundle, path);
      if (path.endsWith(".json")) auditPortableMetadata(JSON.parse(bytes.toString()));
      references.push(fileRef(`${slug}/${path}`, bytes));
    }
    const metadata = references.filter(file => file.media_type === "application/json");
    demos.set(slug, { contract: demoContracts[slug], manifest: metadata.find(file => file.path === `${slug}/manifest.json`)!, metadata: metadata.filter(file => file.path !== `${slug}/manifest.json`) });
    replacement.push(...references);
  }
  const metadata = replacement.filter(file => file.media_type === "application/json");
  const inventory = [...previous.files.filter(file => !items.some(item => file.path.startsWith(`${item.slug}/`))), ...replacement].sort((a, b) => a.path.localeCompare(b.path));
  const release: ShowcaseRelease = { kind: "fx-showcase-release-v1", demos: { ...previous.demos, ...Object.fromEntries(demos) }, files: inventory };
  parseRelease(release);
  const nextBytes = jsonBytes(release), hash = sha256(nextBytes), relative = `showcases/${hash}`, destination = join(delivery, relative);
  const nextLock = structuredClone(lock);
  nextLock.release = { sha256: hash, path: `${relative}/manifest.json`, fixture: `manifests/${hash}.json` };
  // A new showcase starts without recordings; append them separately.
  for (const [slug, demo] of demos) {
    if (additions.includes(slug)) nextLock.workflows[slug] = { demo, recordings: [] };
    else nextLock.workflows[slug].demo = demo;
  }

  await ensureDirectory(join(delivery, "showcases"));
  const staging = await mkdtemp(join(delivery, "showcases/.replace-demo-"));
  const candidate = join(staging, "content"), media = join(staging, "media");
  try {
    for (const file of inventory) {
      const replacement = items.find(item => file.path.startsWith(`${item.slug}/`));
      const bytes = await verifiedFile(replacement ? resolve(replacement.source) : previousRoot, replacement ? { ...file, path: file.path.slice(replacement.slug.length + 1) } : file);
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
  return { showcases: items.map(item => ({ slug: item.slug, added: additions.includes(item.slug) })), release: hash, previous_release: lock.release.sha256, files: inventory.length, delivery_root: delivery, lock: join(content, "lock.json") };
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
