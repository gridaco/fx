import { constants } from "node:fs";
import { parseRunExport } from "@grida/fx-web";
import { copyFile, lstat, mkdtemp, rename, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { auditPortableMetadata, demoContracts, demoResourceFiles, parseCharacter, parseDemo, parseFace, parseParallax, parseSheet, parseTerrainLookup, parseWorld, type DemoRef, type FileRef, type ShowcaseLock, type ShowcaseRelease, type ShowcaseSlug } from "../../src/lib/showcases/contracts";
import { workflowDetails } from "../../src/lib/workflow-details";
import { confinedFile, contentRoot, deliveryRoot, ensureDirectory as mkdir, fileRef, files, jsonBytes, plainWrite as writeFile, sha256, verifiedFile, wwwRoot } from "./io";
import { verifyCampaign } from "./campaign";

function pick(value: Record<string, unknown>, keys: string[]) { return Object.fromEntries(keys.map(key => [key, value[key]])); }
function project(slug: ShowcaseSlug, value: Record<string, unknown>): Record<string, unknown> {
  const kind = demoContracts[slug];
  if (slug === "parallax") return parseParallax({ kind, ...pick(value, ["canvas", "layers"]) });
  if (slug === "world") return parseWorld({ kind, entities: value.entities });
  if (slug === "character-3d") return parseCharacter({ kind, ...pick(value, ["models", "images", "result", "runtime_qualified"]) });
  const resources: string[] = [...demoResourceFiles[slug]];
  if (slug === "portrait-motion") {
    const body = value.body as Record<string, unknown> | undefined;
    const listed = Array.isArray(value.resources) && value.resources.includes("assets/body.webm");
    if (listed || body?.browser_derivative === "assets/body.webm") resources.push("assets/body.webm");
  }
  return { kind, resources };
}
function metadata(path: string, value: unknown): unknown {
  const source = value as Record<string, unknown>;
  if (path.endsWith("face-manifest.json")) return parseFace({ kind: "portrait-patches-v1", ...pick(source, ["patch_application", "feather_already_baked", "source_size", "patch_size", "offset_xy", "timeline", "patches"]) });
  if (path.endsWith("lookup.json")) return parseTerrainLookup({ kind: "terrain-lookup-v1", lookup: source.lookup });
  if (/_(?:validation)\.json$/.test(path)) return parseSheet({ kind: "ui-sheet-v1", ...pick(source, ["role", "canvas", "cells", "insets", "draw_scale", "band_fill"]) });
  return value;
}

/** Preparation reads a previously approved campaign, then creates a smaller website-owned release. */
export async function prepareShowcases(campaignRoot: string, options: { exportsRoot?: string; outputRoot?: string; fixturesRoot?: string; publicRoot?: string } = {}) {
  const output = resolve(options.outputRoot ?? deliveryRoot), fixtureRoot = resolve(options.fixturesRoot ?? contentRoot);
  const campaign = resolve(campaignRoot);
  const inventory = await verifyCampaign(campaign);
  const demos: Record<string, DemoRef> = {}, inventoryFiles: FileRef[] = [];
  const generated = new Map<string, Buffer>(), sources = new Map<string, string>();
  for (const bundle of inventory.bundles) {
    const slug = bundle.showcase as ShowcaseSlug;
    if (!(slug in demoContracts)) throw new Error(`Unknown campaign bundle ${slug}.`);
    const source = join(campaign, "publish", slug);
    const original = JSON.parse((await confinedFile(source, "manifest.json")).toString());
    const manifest = project(slug, original);
    const required = new Set(["cover.webp", ...parseDemo(slug, manifest)]);
    for (const item of workflowDetails[slug].outputs) required.add(item.path);
    for (const ref of workflowDetails[slug].agent) if (typeof ref !== "string" && (!ref.workflow || ref.workflow === slug)) required.add(ref.path);
    // Reviewed assets contain the native overview's inputs and canonical output downloads.
    for (const path of await files(source)) {
      if (/^(?:assets|layers)\//.test(path) && /\.(?:png|jpg|jpeg|webp|mp4|webm|glb)$/.test(path)) required.add(path);
      if (slug === "character-3d" && path.startsWith("vendor/") && !path.endsWith("README.md")) required.add(path);
    }
    generated.set(`${slug}/manifest.json`, jsonBytes(manifest)); required.delete("manifest.json");
    if (slug === "portrait-motion") {
      const facePath = "assets/face-manifest.json";
      const face = metadata(facePath, JSON.parse((await confinedFile(source, facePath)).toString()));
      generated.set(`${slug}/${facePath}`, jsonBytes(face));
      for (const patch of parseFace(face).patches) required.add(`assets/patches/${patch.key}.png`);
    }
    for (const path of required) {
      const relative = `${slug}/${path}`;
      if (!generated.has(relative)) {
        const bytes = await confinedFile(source, path);
        if (path.endsWith(".json")) generated.set(relative, jsonBytes(metadata(path, JSON.parse(bytes.toString()))));
        else sources.set(relative, join(source, path));
      }
    }
    const metadataFiles: FileRef[] = [];
    for (const [path, bytes] of generated) if (path.startsWith(`${slug}/`) && path.endsWith(".json")) metadataFiles.push(fileRef(path, bytes));
    const manifestRef = metadataFiles.find(file => file.path === `${slug}/manifest.json`)!;
    demos[slug] = { contract: demoContracts[slug], manifest: manifestRef, metadata: metadataFiles.filter(file => file !== manifestRef).sort((a, b) => a.path.localeCompare(b.path)) };
  }
  for (const [path, bytes] of generated) { auditPortableMetadata(JSON.parse(bytes.toString())); inventoryFiles.push(fileRef(path, bytes)); }
  for (const [path, source] of sources) inventoryFiles.push(fileRef(path, await confinedFile(dirname(source), source.split("/").at(-1)!)));
  inventoryFiles.sort((a, b) => a.path.localeCompare(b.path));
  const release: ShowcaseRelease = { kind: "fx-showcase-release-v1", demos, files: inventoryFiles };
  const releaseBytes = jsonBytes(release), releaseHash = sha256(releaseBytes);
  const releaseRelative = `showcases/${releaseHash}`, final = join(output, releaseRelative);
  await mkdir(dirname(final), { recursive: true });
  const staging = await mkdtemp(join(dirname(final), ".prepare-"));
  try {
    for (const file of inventoryFiles) {
      await mkdir(dirname(join(staging, file.path)), { recursive: true });
      if (generated.has(file.path)) await writeFile(join(staging, file.path), generated.get(file.path)!);
      else await copyFile(sources.get(file.path)!, join(staging, file.path), constants.COPYFILE_EXCL);
      await verifiedFile(staging, file);
    }
    await writeFile(join(staging, "manifest.json"), releaseBytes);
    const existing = await lstat(final).catch(() => null);
    if (existing) {
      if (sha256(await confinedFile(final, "manifest.json")) !== releaseHash) throw new Error("Immutable release collision.");
      for (const file of inventoryFiles) await verifiedFile(final, file);
    } else await rename(staging, final);
  } finally { await rm(staging, { recursive: true, force: true }); }
  await mkdir(join(fixtureRoot, "manifests"), { recursive: true });
  await writeFile(join(fixtureRoot, "manifests", `${releaseHash}.json`), releaseBytes);
  for (const file of inventoryFiles.filter(file => file.media_type === "application/json")) {
    await mkdir(dirname(join(fixtureRoot, "manifests", file.path)), { recursive: true });
    await writeFile(join(fixtureRoot, "manifests", file.path), await verifiedFile(final, file));
  }
  const lock: ShowcaseLock = { kind: "fx-showcase-lock-v1", release: { sha256: releaseHash, path: `${releaseRelative}/manifest.json`, fixture: `manifests/${releaseHash}.json` }, workflows: {} };
  const recordings: Record<string, [string, string][]> = {
    parallax: [["meadow-01", "Layer preparation"]], world: [["concept-world-01", "World and illustrations"]],
    "movie-sprite": [["movie-sprite-02", "Generated take"], ["movie-sprite-face-pinned-01", "Local finishing"]],
    "portrait-motion": [["portrait-motion-01", "Independent expressions"], ["movie-sprite-face-pinned-01", "Body motion"]],
    "character-3d": [["character-3d-01", "Character construction"]], terrain: [["terrain-01", "Terrain atlas"]], "ui-kit": [["ui-kit-01", "Interface kit"]],
  };
  for (const [slug, demo] of Object.entries(demos)) {
    const refs: ShowcaseLock["workflows"][string]["recordings"] = [];
    if (options.exportsRoot) for (const [id, label] of recordings[slug]) {
      const source = join(options.exportsRoot, id), bytes = await confinedFile(source, "manifest.json"), hash = sha256(bytes);
      const document = JSON.parse(bytes.toString());
      auditPortableMetadata(document);
      const exported = parseRunExport(document);
      const destination = join(output, "runs", hash);
      await mkdir(destination, { recursive: true });
      for (const path of ["manifest.json", ...exported.artifacts.flatMap(item => item.path ? [item.path] : [])]) {
        const data = await confinedFile(source, path), target = join(destination, path);
        if (path !== "manifest.json") {
          const artifact = exported.artifacts.find(item => item.path === path)!;
          if (sha256(data) !== artifact.digest || data.byteLength !== artifact.size) throw new Error(`Changed exported artifact: ${id}/${path}.`);
        }
        await mkdir(dirname(target), { recursive: true });
        const existing = await lstat(target).catch(() => null);
        if (existing && sha256(await confinedFile(destination, path)) !== sha256(data)) throw new Error("Immutable recording collision.");
        if (!existing) await writeFile(target, data);
      }
      await mkdir(join(fixtureRoot, "recordings"), { recursive: true });
      await writeFile(join(fixtureRoot, "recordings", `${hash}.json`), bytes);
      refs.push({ id, label, kind: "fx-run-export-v1", path: `runs/${hash}/manifest.json`, sha256: hash, fixture: `recordings/${hash}.json` });
    }
    lock.workflows[slug] = { demo, recordings: refs };
  }
  await writeFile(join(fixtureRoot, "lock.json"), jsonBytes(lock));
  if (options.publicRoot) {
    for (const path of await files(output)) {
      const bytes = await confinedFile(output, path), destination = join(options.publicRoot, path);
      await mkdir(dirname(destination), { recursive: true }); await writeFile(destination, bytes);
    }
  }
  return { release: releaseHash, files: inventoryFiles.length, bytes: inventoryFiles.reduce((total, file) => total + file.bytes, 0), delivery_root: output, lock: join(fixtureRoot, "lock.json") };
}
if (import.meta.main) {
  const [campaign, exportsRoot] = process.argv.slice(2);
  if (!campaign || !exportsRoot || process.argv.slice(2).length !== 2) throw new Error("Usage: prepare:showcases CAMPAIGN EXPORTS_DIRECTORY");
  console.log(JSON.stringify(await prepareShowcases(campaign, { exportsRoot, publicRoot: join(wwwRoot, "public") }), null, 2));
}
