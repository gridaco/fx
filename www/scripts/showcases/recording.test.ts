import { expect, test } from "bun:test";
import { parseRunExport } from "@grida/fx-web";
import { cp, mkdtemp, readFile, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import original from "../../content/showcases/recordings/8610653e0b02cfb004a05163e512e86bca6b8d0368484a2bf413bfb285b461c3.json";
import { contentRoot, jsonBytes, plainWrite, sha256 } from "./io";
import { appendRecording } from "./recording";

async function fixture(root: string) {
  const content = join(root, "content"), source = join(root, "export"), delivery = join(root, "delivery");
  await cp(contentRoot, content, { recursive: true });
  const document = parseRunExport(structuredClone(original));
  document.view.inputs = {}; document.view.outputs = {};
  for (const node of document.view.nodes) { node.with = {}; node.outputs = {}; }
  const bytes = Buffer.from("recorded-media"), digest = sha256(bytes), path = `artifacts/${digest}.png`;
  document.artifacts = [{ ...document.artifacts[0], digest, media_type: "image/png", size: bytes.length, path, omitted_reason: null }];
  document.view.artifacts = [{ ...document.view.artifacts[0], digest, kind: "image/png", size: bytes.length, available: true }];
  await plainWrite(join(source, "manifest.json"), jsonBytes(document));
  await plainWrite(join(source, path), bytes);
  return { content, source, delivery, path, bytes };
}

test("appending a recording preserves the media release and earlier evidence, and is idempotent", async () => {
  const root = await mkdtemp(join(await realpath(tmpdir()), "fx-recording-"));
  try {
    const item = await fixture(root), before = JSON.parse(await readFile(join(item.content, "lock.json"), "utf8"));
    const result = await appendRecording("parallax", "preview-fixture", "Local preview", item.source, item);
    const after = JSON.parse(await readFile(join(item.content, "lock.json"), "utf8"));
    expect(after.release).toEqual(before.release);
    expect(after.workflows.parallax.recordings.slice(0, -1)).toEqual(before.workflows.parallax.recordings);
    expect(after.workflows.world).toEqual(before.workflows.world);
    expect(await readFile(join(item.delivery, "runs", result.sha256, item.path))).toEqual(item.bytes);
    expect(await appendRecording("parallax", "preview-fixture", "Local preview", item.source, item)).toEqual(result);
    await expect(appendRecording("parallax", "preview-fixture", "Different selection", item.source, item)).rejects.toThrow("already names different");
    await appendRecording("parallax", "preview-fixture", "Local preview", item.source, { ...item, primary: true });
    const primary = JSON.parse(await readFile(join(item.content, "lock.json"), "utf8"));
    expect(primary.workflows.parallax.recordings[0].id).toBe("preview-fixture");
    expect(primary.workflows.parallax.recordings.slice(1)).toEqual(before.workflows.parallax.recordings);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("corrupted media fails before changing the catalog pin", async () => {
  const root = await mkdtemp(join(await realpath(tmpdir()), "fx-recording-corrupt-"));
  try {
    const item = await fixture(root), before = await readFile(join(item.content, "lock.json"));
    await plainWrite(join(item.source, item.path), Buffer.from("corrupt"));
    await expect(appendRecording("parallax", "preview-fixture", "Local preview", item.source, item)).rejects.toThrow("Changed exported artifact");
    expect(await readFile(join(item.content, "lock.json"))).toEqual(before);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("private recording labels are refused before writing content", async () => {
  const root = await mkdtemp(join(await realpath(tmpdir()), "fx-recording-private-"));
  try {
    const item = await fixture(root), before = await readFile(join(item.content, "lock.json"));
    await expect(appendRecording("parallax", "preview-fixture", "From /Users/private/work", item.source, item)).rejects.toThrow("private path");
    expect(await readFile(join(item.content, "lock.json"))).toEqual(before);
  } finally { await rm(root, { recursive: true, force: true }); }
});
