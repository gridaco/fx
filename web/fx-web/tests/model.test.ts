import { describe, expect, test } from "bun:test";
import { modelFixture } from "../../../tools/make_model_fixture.mjs";
import { MODEL_BYTE_LIMIT, ModelSourceController, readModel, validateModel } from "../src/model";
import type { Artifact } from "../src/index";
import { readFileSync } from "node:fs";
import { exportArtifactUrl, parseRunExport } from "../src/export";

const buffer = (bytes: Uint8Array): ArrayBuffer => Uint8Array.from(bytes).buffer;
const fixture = (options = {}): ArrayBuffer => buffer(modelFixture(options));
function rewrite(bytes: ArrayBuffer, edit: (document: any) => void): ArrayBuffer {
  const view = new DataView(bytes), end = 20 + view.getUint32(12, true);
  const document = JSON.parse(new TextDecoder().decode(new Uint8Array(bytes, 20, end - 20)));
  edit(document);
  const json = new TextEncoder().encode(JSON.stringify(document)), size = Math.ceil(json.byteLength / 4) * 4;
  const result = new Uint8Array(20 + size + bytes.byteLength - end), header = new DataView(result.buffer);
  result.set(new Uint8Array(bytes, 0, 20)); header.setUint32(8, result.byteLength, true); header.setUint32(12, size, true);
  result.fill(32, 20, 20 + size); result.set(json, 20); result.set(new Uint8Array(bytes, end), 20 + size);
  return result.buffer;
}
async function artifact(bytes: ArrayBuffer): Promise<Artifact> {
  const digest = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), (byte) => byte.toString(16).padStart(2, "0")).join("");
  return { kind: "model/gltf-binary", digest, name: "sample.glb", size: bytes.byteLength, available: true, url: `/api/artifacts/${digest}` };
}

describe("recorded GLB boundary", () => {
  test("original static, rigged and animated self-contained fixtures validate without mutation", () => {
    for (const options of [{}, { animated: false, skinned: false }, { animated: false, skinned: true }, { dataUris: true }]) {
      const bytes = fixture(options), before = bytes.slice(0);
      validateModel(bytes);
      expect(new Uint8Array(bytes)).toEqual(new Uint8Array(before));
    }
  });
  test("refuses external resources, required extensions, and a scene containing motion without a mesh", () => {
    expect(() => validateModel(fixture({ external: true }))).toThrow("embedded resources");
    expect(() => validateModel(fixture({ requiredExtension: "ACME_unsupported" }))).toThrow("unsupported glTF extension");
    expect(() => validateModel(fixture({ motionOnly: true }))).toThrow("no mesh");
  });
  test("refuses invalid header, declared length, JSON and truncated chunks", () => {
    const bytes = fixture();
    for (const offset of [0, 4, 8, 12, 16, 20]) {
      const changed = bytes.slice(0); new Uint8Array(changed)[offset] ^= 0xff;
      expect(() => validateModel(changed)).toThrow("valid GLB 2");
    }
    expect(() => validateModel(bytes.slice(0, bytes.byteLength - 1))).toThrow("valid GLB 2");
  });
  test("blocks relative/active data resources even inside optional extensions, and cyclic scenes", () => {
    for (const uri of ["texture.png", "/api/view", "file:///local.png", "data:image/svg+xml;base64,PHN2Zz4=", "https://acme.invalid/image.png"]) {
      const bytes = rewrite(fixture(), (document) => { document.extensions = { ACME_optional: { uri } }; });
      expect(() => validateModel(bytes)).toThrow("embedded resources");
    }
    expect(() => validateModel(rewrite(fixture(), (document) => { document.nodes[2].children = [1]; }))).toThrow("valid GLB 2");
  });
  test("verifies exact bytes and refuses mismatched inventory without exposing file content", async () => {
    const bytes = fixture(), recorded = await artifact(bytes);
    const fetcher = (async (_url: unknown, options: RequestInit) => {
      expect(options.redirect).toBe("error");
      return new Response(bytes);
    }) as typeof fetch;
    expect(await readModel(recorded, new AbortController().signal, fetcher)).toEqual(bytes);
    await expect(readModel({ ...recorded, size: recorded.size + 1 }, new AbortController().signal, fetcher)).rejects.toThrow("recorded size");
    await expect(readModel({ ...recorded, digest: "a".repeat(64) }, new AbortController().signal, fetcher)).rejects.toThrow("recorded digest");
  });
  test("refuses over-limit and unavailable records before fetching, and cancels an oversized stream", async () => {
    const recorded = await artifact(fixture());
    let calls = 0, cancelled = false;
    const fetcher = (async () => { calls++; return new Response(new ReadableStream({
      start(controller) { controller.enqueue(new Uint8Array(recorded.size + 1)); },
      cancel() { cancelled = true; },
    })); }) as typeof fetch;
    await expect(readModel({ ...recorded, size: MODEL_BYTE_LIMIT + 1 }, new AbortController().signal, fetcher)).rejects.toThrow("128 MiB");
    await expect(readModel({ ...recorded, available: false }, new AbortController().signal, fetcher)).rejects.toThrow("unavailable");
    expect(calls).toBe(0);
    await expect(readModel(recorded, new AbortController().signal, fetcher)).rejects.toThrow("recorded size");
    expect(cancelled).toBe(true);
  });
  test("included portable GLB exports use the same verified byte reader and trusted delivery base", async () => {
    const bytes = fixture(), recorded = await artifact(bytes);
    const manifest = JSON.parse(readFileSync(new URL("../../../spec/vectors/exports/minimal.json", import.meta.url), "utf8"));
    manifest.artifacts = [{ digest: recorded.digest, media_type: recorded.kind, name: recorded.name, size: recorded.size, path: `artifacts/${recorded.digest}.glb`, omitted_reason: null }];
    manifest.view.artifacts = [{ ...recorded, url: null }];
    manifest.view.nodes[0].outputs = { model: { file: { digest: recorded.digest, kind: recorded.kind, name: recorded.name, size: recorded.size } } };
    const recording = { manifest: parseRunExport(manifest), manifest_url: "https://assets.example.com/review/manifest.json" };
    const url = exportArtifactUrl(recording, recorded.digest)!;
    expect(url).toBe(`https://assets.example.com/review/artifacts/${recorded.digest}.glb`);
    const fetcher = (async (input: unknown) => { expect(input).toBe(url); return new Response(bytes); }) as typeof fetch;
    expect(await readModel({ ...recorded, url }, new AbortController().signal, fetcher)).toEqual(bytes);
  });
});

test("replaced and disposed selections cannot publish late URLs, and owned URLs are revoked once", async () => {
  const recorded = await artifact(fixture());
  const pending: { signal: AbortSignal; resolve: (bytes: ArrayBuffer) => void }[] = [];
  const revoked: string[] = [];
  let count = 0;
  const source = new ModelSourceController({
    read: (_artifact, signal) => new Promise((resolve) => pending.push({ signal, resolve })),
    createUrl: () => `blob:fixture-${++count}`,
    revokeUrl: (url) => revoked.push(url),
  });
  const old = source.load(recorded), current = source.load(recorded);
  expect(pending[0].signal.aborted).toBe(true);
  pending[1].resolve(fixture()); await current;
  expect(source.getSnapshot().url).toBe("blob:fixture-1");
  pending[0].resolve(fixture()); await old;
  expect(source.getSnapshot().url).toBe("blob:fixture-1");
  const disposed = source.load(recorded);
  expect(revoked).toEqual(["blob:fixture-1"]);
  source.dispose(); pending[2].resolve(fixture()); await disposed;
  expect(count).toBe(1);
  expect(source.getSnapshot().phase).toBe("idle");
  source.dispose(); expect(revoked).toEqual(["blob:fixture-1"]);
});
