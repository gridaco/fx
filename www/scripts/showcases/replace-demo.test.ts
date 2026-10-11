import { expect, test } from "bun:test";
import { cp, mkdtemp, readFile, realpath, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseLock, parseRelease, type ShowcaseSlug } from "../../src/lib/showcases/contracts";
import { confinedFile, contentRoot, fileRef, jsonBytes, plainWrite, sha256, verifiedFile } from "./io";
import { replaceDemo } from "./replace-demo";
import { verifyShowcases } from "./verify";
import { workflowDetails } from "../../src/lib/workflow-details";

/** Codec-free media makes the release integrity tests independent of generated artwork. */
async function fixture(root: string, slug: ShowcaseSlug = "movie-sprite") {
  const content = join(root, "content"), delivery = join(root, "delivery"), source = join(root, "bundle");
  await cp(contentRoot, content, { recursive: true });
  const lock = parseLock(JSON.parse((await confinedFile(content, "lock.json")).toString()));
  const original = parseRelease(JSON.parse((await confinedFile(content, lock.release.fixture)).toString()));
  const data = new Map<string, Buffer>();
  const inventory = [];
  for (const file of original.files) {
    const bytes = file.media_type === "application/json" ? await verifiedFile(join(content, "manifests"), file) : Buffer.from(`synthetic:${file.path}`);
    data.set(file.path, bytes);
    inventory.push(fileRef(file.path, bytes));
  }
  const refs = new Map(inventory.map(file => [file.path, file]));
  const release = { ...original, files: inventory, demos: Object.fromEntries(Object.entries(original.demos).map(([id, demo]) => [id, { ...demo, manifest: refs.get(demo.manifest.path)!, metadata: demo.metadata.map(file => refs.get(file.path)!) }])) };
  const bytes = jsonBytes(release), hash = sha256(bytes), previousRoot = join(delivery, "showcases", hash);
  for (const [path, bytes] of data) {
    await plainWrite(join(previousRoot, path), bytes);
    if (path.startsWith(`${slug}/`)) await plainWrite(join(source, path.slice(slug.length + 1)), bytes);
  }
  await plainWrite(join(previousRoot, "manifest.json"), bytes);
  await plainWrite(join(content, "manifests", `${hash}.json`), bytes);
  lock.release = { sha256: hash, path: `showcases/${hash}/manifest.json`, fixture: `manifests/${hash}.json` };
  for (const [id, demo] of Object.entries(release.demos)) lock.workflows[id].demo = demo;
  await plainWrite(join(content, "lock.json"), jsonBytes(lock));
  await plainWrite(join(source, "cover.webp"), Buffer.from("replacement-cover"));
  return { content, delivery, source, slug, lock, previousRoot };
}

test("replacing one demo preserves all other demos, recordings and their selected order", async () => {
  const root = await mkdtemp(join(await realpath(tmpdir()), "fx-replace-demo-"));
  try {
    const item = await fixture(root), before = await confinedFile(item.content, "lock.json");
    const result = await replaceDemo(item.slug, item.source, item);
    const after = parseLock(JSON.parse((await confinedFile(item.content, "lock.json")).toString()));
    expect(result.release).not.toBe(item.lock.release.sha256);
    for (const [slug, workflow] of Object.entries(item.lock.workflows)) {
      expect(after.workflows[slug].recordings).toEqual(workflow.recordings);
      if (slug !== item.slug) expect(after.workflows[slug]).toEqual(workflow);
    }
    expect(await confinedFile(item.previousRoot, "manifest.json")).toEqual(await confinedFile(item.content, item.lock.release.fixture));
    const destination = join(item.delivery, "showcases", result.release);
    expect(await confinedFile(destination, `${item.slug}/cover.webp`)).toEqual(Buffer.from("replacement-cover"));
    const release = parseRelease(JSON.parse((await confinedFile(destination, "manifest.json")).toString()));
    for (const file of release.files) await verifiedFile(destination, file);
    await expect(verifyShowcases({ content: item.content, requireRecordings: true })).resolves.toHaveProperty("release", result.release);
    const firstLock = await confinedFile(item.content, "lock.json");
    expect((await replaceDemo(item.slug, item.source, item)).release).toBe(result.release);
    expect(await confinedFile(item.content, "lock.json")).toEqual(firstLock);
    expect(firstLock).not.toEqual(before);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("changed frozen or staged evidence fails without changing the catalog or metadata", async () => {
  for (const target of ["frozen", "staged"]) {
    const root = await mkdtemp(join(await realpath(tmpdir()), "fx-replace-corrupt-"));
    try {
      const item = await fixture(root), before = await confinedFile(item.content, "lock.json"), metadata = await confinedFile(item.content, "manifests/movie-sprite/manifest.json");
      await plainWrite(target === "frozen" ? join(item.content, item.lock.release.fixture) : join(item.previousRoot, "world/cover.webp"), Buffer.from("corrupt"));
      await expect(replaceDemo(item.slug, item.source, item)).rejects.toThrow(target === "frozen" ? "Frozen release" : "Content changed");
      expect(await confinedFile(item.content, "lock.json")).toEqual(before);
      expect(await confinedFile(item.content, "manifests/movie-sprite/manifest.json")).toEqual(metadata);
    } finally { await rm(root, { recursive: true, force: true }); }
  }
});

test("invalid, missing and private bundle resources fail before replacing current content", async () => {
  for (const failure of ["missing", "schema", "private", "symlink", "patch"]) {
    const root = await mkdtemp(join(await realpath(tmpdir()), "fx-replace-invalid-"));
    try {
      const item = await fixture(root, failure === "patch" ? "portrait-motion" : "movie-sprite");
      const before = await confinedFile(item.content, "lock.json"), metadata = await confinedFile(item.content, `manifests/${item.slug}/manifest.json`);
      if (failure === "missing") await rm(join(item.source, "assets/canonical.png"));
      else if (failure === "schema") await plainWrite(join(item.source, "manifest.json"), jsonBytes({ kind: "unsupported" }));
      else if (failure === "private") await plainWrite(join(item.source, "review.json"), jsonBytes({ source: "/Users/private/art.png" }));
      else if (failure === "symlink") await symlink(join(item.source, "cover.webp"), join(item.source, "linked.webp"));
      else {
        const face = JSON.parse(await readFile(join(item.source, "assets/face-manifest.json"), "utf8"));
        await rm(join(item.source, `assets/patches/${face.patches[0].key}.png`));
      }
      await expect(replaceDemo(item.slug, item.source, item)).rejects.toThrow();
      expect(await confinedFile(item.content, "lock.json")).toEqual(before);
      expect(await confinedFile(item.content, `manifests/${item.slug}/manifest.json`)).toEqual(metadata);
    } finally { await rm(root, { recursive: true, force: true }); }
  }
});


test("replacement validates new page downloads against its candidate without requiring them in the old release", async () => {
  const root = await mkdtemp(join(await realpath(tmpdir()), "fx-replace-page-assets-"));
  const before = workflowDetails["movie-sprite"].outputs;
  try {
    const item = await fixture(root);
    workflowDetails["movie-sprite"].outputs = [...before, { path: "assets/new-record.md", description: "New recorded evidence" }];
    await expect(verifyShowcases({ content: item.content })).rejects.toThrow("Missing page asset movie-sprite/assets/new-record.md");
    const lock = await confinedFile(item.content, "lock.json");
    await expect(replaceDemo(item.slug, item.source, item)).rejects.toThrow("Missing page asset movie-sprite/assets/new-record.md");
    expect(await confinedFile(item.content, "lock.json")).toEqual(lock);
    await plainWrite(join(item.source, "assets/new-record.md"), Buffer.from("new evidence"));
    const result = await replaceDemo(item.slug, item.source, item);
    await expect(verifyShowcases({ content: item.content })).resolves.toHaveProperty("release", result.release);
  } finally {
    workflowDetails["movie-sprite"].outputs = before;
    await rm(root, { recursive: true, force: true });
  }
});
