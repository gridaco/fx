import { expect, test } from "bun:test";
import { FileInventoryController, artifactFileTarget, parseFileInventory, type FileDescriptor } from "../src/files";

const digest = "a".repeat(64);
const artifact = { digest, name: "image.png", kind: "image/png", size: 4, available: true, url: `/api/artifacts/${digest}` };
const source: FileDescriptor = { role: "node", instance_id: "draw#1", target: { kind: "source", source: "nodes/draw.py", symbol: "draw", recorded_digest: digest } };

test("artifact targets match native portable names without forwarding private recorded labels", () => {
  for (const name of ["file:original.png", "FiLe:original.png", "/private/image.png", "\\private\\image.png", "C:private.png", "z:\\private\\image.png", "image\u0000.png", "image\u001f.png", "image\u007f.png", "image\u0085.png", "image\u009f.png"]) {
    const target = artifactFileTarget({ ...artifact, name });
    expect(target).toEqual({ kind: "artifact", artifact: { digest, url: artifact.url, name: digest, kind: artifact.kind, size: artifact.size } });
    expect(parseFileInventory({ kind: "fx-files-v1", files: [{ role: "artifact", target }] })).toHaveLength(1);
  }
  for (const name of ["", "image.png", "relative/image.png", "original: image.png", "판넬.png"]) {
    expect(artifactFileTarget({ ...artifact, name })).toEqual({ kind: "artifact", artifact: { digest, url: artifact.url, name, kind: artifact.kind, size: artifact.size } });
  }
});

test("portable file inventory validates scoped artifact URLs and source references", () => {
  const document = { kind: "fx-files-v1", files: [{ role: "artifact", target: artifactFileTarget(artifact) }, source] };
  const parsed = parseFileInventory(document);
  expect(parsed).toEqual(document.files);
  document.files.length = 0;
  expect(parsed).toHaveLength(2);
  const base = `/p/${"b".repeat(64)}/runs/${"c".repeat(64)}/api`;
  expect(() => parseFileInventory({ kind: "fx-files-v1", files: [{ role: "artifact", target: artifactFileTarget(artifact) }] }, base)).toThrow("unscoped");
  expect(parseFileInventory({ kind: "fx-files-v1", files: [{ role: "artifact", target: artifactFileTarget({ ...artifact, url: `${base}/artifacts/${digest}` }) }] }, base)).toHaveLength(1);
  for (const source of ["/private/file.py", "../file.py", "file:///private/file.py", "nodes\\file.py"]) {
    expect(() => parseFileInventory({ kind: "fx-files-v1", files: [{ role: "node", target: { kind: "source", source } }] })).toThrow("malformed");
  }
  expect(() => parseFileInventory({ kind: "fx-file-resolution-v1", local_path: "/private/file.py" })).toThrow();
  expect(() => parseFileInventory({ kind: "fx-files-v1", files: [{ ...source, local_path: "/private/file.py" }] })).toThrow("malformed");
  expect(() => parseFileInventory({ kind: "fx-files-v1", files: [{ ...source, native_result: { local_path: "/private/file.py" } }] })).toThrow("malformed");
});

test("inventory cancellation and cleanup discard stale source metadata", async () => {
  const reads: { signal: AbortSignal; resolve(value: FileDescriptor[]): void }[] = [];
  const inventory = new FileInventoryController((signal) => new Promise((resolve) => { reads.push({ signal, resolve }); }));
  const first = inventory.refresh();
  const second = inventory.refresh();
  expect(reads[0].signal.aborted).toBeTrue();
  reads[1].resolve([source]); await second;
  reads[0].resolve([]); await first;
  expect(inventory.getSnapshot().files).toEqual([source]);
  const third = inventory.refresh();
  inventory.clear();
  expect(reads[2].signal.aborted).toBeTrue();
  reads[2].resolve([source]); await third;
  expect(inventory.getSnapshot().files).toEqual([]);
  inventory.dispose();
  await inventory.refresh();
  expect(reads).toHaveLength(3);
});

test("inventory metadata failures expose safe text and an explicit refresh recovers source discovery", async () => {
  let reads = 0;
  const inventory = new FileInventoryController(async () => {
    if (++reads === 1) throw new Error("secret /private/location");
    return [source];
  });
  await inventory.refresh();
  expect(inventory.getSnapshot()).toEqual({ files: [], loading: false, error: "The recorded source files are unavailable." });
  await inventory.refresh();
  expect(reads).toBe(2);
  expect(inventory.getSnapshot()).toEqual({ files: [source], loading: false, error: null });
  inventory.dispose();
});
