import { parseRunExport } from "@grida/fx-web";
import { basename, dirname, join, resolve } from "node:path";
import { auditPortableMetadata, demoContracts, parseDemo, parseFace, parseLock, parseRelease, parseSheet, parseTerrainLookup, safePath, type FileRef, type ShowcaseSlug } from "../../src/lib/showcases/contracts";
import { workflowDetails } from "../../src/lib/workflow-details";
import { buildFiles, confinedBuildFile, confinedFile, contentRoot, deliveryRoot, jsonBytes, plainWrite as writeFile, sha256, verifiedFile, wwwRoot } from "./io";

function parseMetadata(path: string, value: unknown) {
  if (path.endsWith("face-manifest.json")) parseFace(value);
  else if (path.endsWith("lookup.json")) parseTerrainLookup(value);
  else if (path.endsWith("_validation.json")) parseSheet(value);
}
export async function verifyShowcases(options: { content?: string; delivery?: string; requireRecordings?: boolean } = {}) {
  const content = options.content ?? contentRoot;
  const lockBytes = await confinedFile(content, "lock.json"), lock = parseLock(JSON.parse(lockBytes.toString()));
  const releaseBytes = await confinedFile(content, lock.release.fixture);
  if (sha256(releaseBytes) !== lock.release.sha256) throw new Error("Frozen release manifest changed.");
  const release = parseRelease(JSON.parse(releaseBytes.toString()));
  if (Object.keys(lock.workflows).sort().join() !== Object.keys(demoContracts).sort().join() || Object.keys(release.demos).sort().join() !== Object.keys(demoContracts).sort().join()) throw new Error("The catalog must cover all seven showcase adapters.");
  let metadataCount = 0, recordingsCount = 0;
  const allFiles: FileRef[] = release.files.map(file => ({ ...file, path: `showcases/${lock.release.sha256}/${file.path}` }));
  allFiles.push({ path: lock.release.path, sha256: lock.release.sha256, bytes: releaseBytes.byteLength, media_type: "application/json" });
  for (const [slug, item] of Object.entries(lock.workflows)) {
    if (JSON.stringify(item.demo) !== JSON.stringify(release.demos[slug])) throw new Error("Catalog demo differs from pinned release.");
    const metadata = [item.demo.manifest, ...item.demo.metadata];
    const details = workflowDetails[slug as ShowcaseSlug];
    const required = [`${slug}/cover.webp`, ...details.outputs.map(item => `${slug}/${item.path}`), ...details.agent.filter(item => typeof item !== "string").map(item => `${item.workflow ?? slug}/${item.path}`)];
    for (const path of required) if (!release.files.some(file => file.path === path)) throw new Error(`Missing page asset ${path}.`);
    for (const file of metadata) {
      const bytes = await verifiedFile(join(content, "manifests"), file);
      const value = JSON.parse(bytes.toString());
      auditPortableMetadata(value);
      if (file.path === item.demo.manifest.path) {
        for (const path of parseDemo(slug as ShowcaseSlug, value)) if (!release.files.some(file => file.path === `${slug}/${path}`)) throw new Error(`Missing demo resource ${slug}/${path}.`);
      } else {
        parseMetadata(file.path, value);
        if (file.path.endsWith("face-manifest.json")) for (const patch of parseFace(value).patches) {
          if (!release.files.some(file => file.path === `${slug}/assets/patches/${patch.key}.png`)) throw new Error("Missing portrait patch.");
        }
      }
      metadataCount++;
    }
    if (options.requireRecordings && !item.recordings.length) throw new Error(`No recording selected for ${slug}.`);
    for (const recording of item.recordings) {
      const bytes = await confinedFile(content, recording.fixture);
      if (sha256(bytes) !== recording.sha256) throw new Error("Frozen recording changed.");
      const exported = parseRunExport(JSON.parse(bytes.toString()));
      auditPortableMetadata(exported);
      allFiles.push({ path: recording.path, sha256: recording.sha256, bytes: bytes.byteLength, media_type: "application/json" });
      for (const artifact of exported.artifacts) if (artifact.path) allFiles.push({ path: `${recording.path.slice(0, -"manifest.json".length)}${safePath(artifact.path)}`, sha256: artifact.digest, bytes: artifact.size, media_type: artifact.media_type });
      recordingsCount++;
    }
  }
  const selected = [...new Map(allFiles.map(file => [file.path, file])).values()].sort((a, b) => a.path.localeCompare(b.path));
  if (options.delivery) for (const file of selected) await verifiedFile(options.delivery, file);
  return { lock_sha256: sha256(lockBytes), release: lock.release.sha256, metadata: metadataCount, recordings: recordingsCount, files: selected };
}

type BuildBinding = { kind: "fx-showcase-build-v1"; lock_sha256: string; delivery_base: string; output_sha256: string; output: { path: string; sha256: string }[] };
async function outputInventory(output: string) {
  const paths = await buildFiles(output);
  return Promise.all(paths.map(async path => ({ path, sha256: sha256(await confinedBuildFile(output, path)) })));
}
/** Called only after a successful build, using the exact base passed to that build. */
export async function writeBuildBinding(deliveryBase: string, options: { output?: string; path?: string; expectedLock?: string } = {}): Promise<BuildBinding> {
  const verified = await verifyShowcases({ requireRecordings: true });
  if (options.expectedLock && options.expectedLock !== verified.lock_sha256) throw new Error("Content lock changed during the website build.");
  const output = await outputInventory(options.output ?? join(wwwRoot, "out"));
  const binding: BuildBinding = { kind: "fx-showcase-build-v1", lock_sha256: verified.lock_sha256, delivery_base: deliveryBase.replace(/\/+$/, ""), output_sha256: sha256(jsonBytes(output)), output };
  await writeFile(options.path ?? join(wwwRoot, ".showcases/build.json"), jsonBytes(binding));
  return binding;
}
export async function verifyBuildBinding(deliveryBase?: string, options: { output?: string; path?: string } = {}): Promise<BuildBinding> {
  const path = options.path ?? join(wwwRoot, ".showcases/build.json");
  const binding = JSON.parse((await confinedFile(dirname(path), basename(path))).toString()) as BuildBinding;
  const verified = await verifyShowcases({ requireRecordings: true });
  const output = await outputInventory(options.output ?? join(wwwRoot, "out"));
  if (binding.kind !== "fx-showcase-build-v1" || binding.lock_sha256 !== verified.lock_sha256 || binding.output_sha256 !== sha256(jsonBytes(output))) throw new Error("Website output or content lock changed since the successful build. Rebuild before qualifying this candidate.");
  if (deliveryBase !== undefined && deliveryBase.replace(/\/+$/, "") !== binding.delivery_base) throw new Error("Candidate delivery base differs from the successful website build.");
  return binding;
}

/** Bind checks to the built static output, not a prior successful source check. */
export async function candidateReceipt(deliveryBase?: string, output = join(wwwRoot, "out"), delivery = deliveryRoot) {
  const verified = await verifyShowcases({ delivery, requireRecordings: true });
  const binding = await verifyBuildBinding(deliveryBase, { output });
  const receipt = { kind: "fx-showcase-candidate-v1", lock_sha256: verified.lock_sha256, release: verified.release, delivery_base: binding.delivery_base, output_sha256: binding.output_sha256, output: binding.output,
    adapters: Object.values(demoContracts), recordings: verified.recordings, browser_acceptance: "pending" };
  const destination = join(wwwRoot, ".showcases/candidate.json");
  await writeFile(destination, jsonBytes(receipt));
  return { ...receipt, receipt: destination };
}
if (import.meta.main) {
  const args = process.argv.slice(2);
  if (args[0] === "--candidate") {
    const receipt = await candidateReceipt(args[1]);
    console.log(JSON.stringify({ ...receipt, output: receipt.output.length }, null, 2));
  }
  else {
    const media = args.indexOf("--media");
    const mediaPath = media >= 0 && args[media + 1] && !args[media + 1].startsWith("--") ? args[media + 1] : deliveryRoot;
    if (args.some(arg => arg.startsWith("--") && !["--media", "--recordings"].includes(arg))) throw new Error("Usage: verify:showcases [--media [DIRECTORY]] [--recordings]");
    const result = await verifyShowcases({ delivery: media >= 0 ? resolve(mediaPath) : undefined, requireRecordings: args.includes("--recordings") });
    console.log(JSON.stringify({ ...result, files: result.files.length }, null, 2));
  }
}
