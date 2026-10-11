import { parseRunExport } from "@grida/fx-web";
import { lstat, mkdtemp, rename, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { auditPortableMetadata, parseLock, type ShowcaseSlug } from "../../src/lib/showcases/contracts";
import { confinedFile, contentRoot, deliveryRoot, ensureDirectory, jsonBytes, plainWrite, sha256, wwwRoot } from "./io";

/** Append a reviewed export without rebuilding or mutating the existing media release. */
export async function appendRecording(slug: ShowcaseSlug, id: string, label: string, source: string, options: { content?: string; delivery?: string; publicRoot?: string; primary?: boolean } = {}) {
  if (!/^[a-z0-9][a-z0-9_-]*$/.test(id) || !label.trim()) throw new Error("A recording needs a portable ID and label.");
  auditPortableMetadata(label);
  const content = options.content ?? contentRoot, delivery = options.delivery ?? deliveryRoot;
  const lockPath = join(content, "lock.json"), lockBytes = await confinedFile(content, "lock.json");
  const lock = parseLock(JSON.parse(lockBytes.toString()));
  if (!(slug in lock.workflows)) throw new Error(`Unknown showcase ${slug}.`);
  const manifest = await confinedFile(resolve(source), "manifest.json"), hash = sha256(manifest);
  const document = JSON.parse(manifest.toString());
  auditPortableMetadata(document);
  const exported = parseRunExport(document);
  const previous = lock.workflows[slug].recordings.find(item => item.id === id);
  if (previous && (previous.sha256 !== hash || previous.label !== label)) throw new Error("Recording ID already names different evidence; use a new ID.");
  const relative = `runs/${hash}`, destination = join(delivery, relative);
  await ensureDirectory(dirname(destination));
  const staging = await mkdtemp(join(dirname(destination), ".recording-"));
  const paths = ["manifest.json", ...exported.artifacts.flatMap(item => item.path ? [item.path] : [])];
  try {
    for (const path of paths) {
      const bytes = path === "manifest.json" ? manifest : await confinedFile(resolve(source), path);
      if (path !== "manifest.json") {
        const artifact = exported.artifacts.find(item => item.path === path)!;
        if (sha256(bytes) !== artifact.digest || bytes.byteLength !== artifact.size) throw new Error(`Changed exported artifact: ${path}.`);
      }
      await plainWrite(join(staging, path), bytes);
    }
    const existing = await lstat(destination).catch((error: NodeJS.ErrnoException) => { if (error.code === "ENOENT") return null; throw error; });
    if (existing) {
      for (const path of paths) if (sha256(await confinedFile(destination, path)) !== sha256(await confinedFile(staging, path))) throw new Error("Immutable recording collision.");
    } else await rename(staging, destination);
  } finally { await rm(staging, { recursive: true, force: true }); }
  if (options.publicRoot) for (const path of paths) {
    const bytes = await confinedFile(destination, path), target = join(options.publicRoot, relative, path);
    const existing = await lstat(target).catch((error: NodeJS.ErrnoException) => { if (error.code === "ENOENT") return null; throw error; });
    if (existing && sha256(await confinedFile(options.publicRoot, `${relative}/${path}`)) !== sha256(bytes)) throw new Error("Immutable public recording collision.");
    if (!existing) await plainWrite(target, bytes);
  }
  const fixture = `recordings/${hash}.json`;
  const existingFixture = await lstat(join(content, fixture)).catch((error: NodeJS.ErrnoException) => { if (error.code === "ENOENT") return null; throw error; });
  if (existingFixture && sha256(await confinedFile(content, fixture)) !== hash) throw new Error("Frozen recording collision.");
  if (!existingFixture) await plainWrite(join(content, fixture), manifest);
  if (!previous || options.primary) {
    const ref = previous ?? { id, label, kind: "fx-run-export-v1" as const, path: `${relative}/manifest.json`, sha256: hash, fixture };
    if (options.primary) lock.workflows[slug].recordings = [ref, ...lock.workflows[slug].recordings.filter(item => item.id !== id)];
    else lock.workflows[slug].recordings.push(ref);
    const temporary = await mkdtemp(join(content, ".recording-lock-"));
    try {
      await plainWrite(join(temporary, "lock.json"), jsonBytes(lock));
      if (sha256(await confinedFile(content, "lock.json")) !== sha256(lockBytes)) throw new Error("Content lock changed during recording preparation.");
      await rename(join(temporary, "lock.json"), lockPath);
    } finally { await rm(temporary, { recursive: true, force: true }); }
  }
  return { id, sha256: hash, files: paths.length, path: `${relative}/manifest.json` };
}

if (import.meta.main) {
  const [slug, id, label, source, ...extra] = process.argv.slice(2);
  if (!slug || !id || !label || !source || extra.some(arg => arg !== "--primary") || extra.length > 1) throw new Error("Usage: prepare:recording SHOWCASE ID LABEL EXPORTED_RUN [--primary]");
  console.log(JSON.stringify(await appendRecording(slug as ShowcaseSlug, id, label, source, { publicRoot: join(wwwRoot, "public"), primary: extra.includes("--primary") }), null, 2));
}
