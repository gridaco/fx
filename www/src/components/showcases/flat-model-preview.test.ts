import { expect, test } from "bun:test";
import { FlatModelPreview, flatModelBytes } from "./flat-model-preview";

function glb(document: unknown): ArrayBuffer {
  const json = new TextEncoder().encode(JSON.stringify(document)), size = Math.ceil(json.byteLength / 4) * 4;
  const bytes = new Uint8Array(20 + size + 16), header = new DataView(bytes.buffer);
  header.setUint32(0, 0x46546c67, true); header.setUint32(4, 2, true); header.setUint32(8, bytes.byteLength, true);
  header.setUint32(12, size, true); header.setUint32(16, 0x4e4f534a, true);
  bytes.fill(32, 20, 20 + size); bytes.set(json, 20);
  header.setUint32(20 + size, 8, true); header.setUint32(24 + size, 0x004e4942, true);
  bytes.set([0, 1, 2, 3, 250, 251, 252, 253], 28 + size);
  return bytes.buffer;
}

const document = {
  asset: { version: "2.0" }, buffers: [{ byteLength: 8 }], images: [{ bufferView: 0, mimeType: "image/png" }],
  skins: [{ joints: [0, 1] }], animations: [{ name: "cheer", channels: [], samplers: [] }],
  extensionsUsed: ["KHR_materials_specular"],
  materials: [{ doubleSided: true, alphaMode: "MASK", alphaCutoff: 0.3,
    pbrMetallicRoughness: { baseColorFactor: [0.4, 0.5, 0.6, 0.7], baseColorTexture: { index: 0, texCoord: 1 } },
    extensions: { KHR_materials_specular: { specularFactor: 0.08 } } }],
};

test("flat preview preserves source bytes, binary chunks, texture colors, alpha, skinning and animations", () => {
  const source = glb(document), before = new Uint8Array(source).slice(), result = flatModelBytes(source);
  const header = new DataView(result.buffer), size = header.getUint32(12, true);
  expect(header.getUint32(8, true)).toBe(result.byteLength);
  expect(size % 4).toBe(0);
  const actual = JSON.parse(new TextDecoder().decode(result.subarray(20, 20 + size)));
  expect(actual.extensionsRequired).toEqual(["KHR_materials_unlit"]);
  expect(actual.extensionsUsed).toEqual(["KHR_materials_specular", "KHR_materials_unlit"]);
  expect(actual.materials[0].extensions.KHR_materials_unlit).toEqual({});
  delete actual.materials[0].extensions.KHR_materials_unlit;
  delete actual.extensionsRequired;
  actual.extensionsUsed.pop();
  expect(actual).toEqual(document);
  expect(result.subarray(20 + size)).toEqual(before.subarray(20 + new DataView(source).getUint32(12, true)));
  expect(new Uint8Array(source)).toEqual(before);
});

test("preview refuses invalid GLB containers and resources that a temporary blob cannot resolve", () => {
  expect(() => flatModelBytes(new ArrayBuffer(8))).toThrow("valid GLB");
  const truncated = glb(document).slice(0, -1);
  new DataView(truncated).setUint32(8, truncated.byteLength, true);
  expect(() => flatModelBytes(truncated)).toThrow("invalid chunk length");
  expect(() => flatModelBytes(glb({ ...document, images: [{ uri: "texture.png" }] }))).toThrow("embedded model resources");
  expect(() => flatModelBytes(glb({ ...document, buffers: [{ uri: "https://other.invalid/model.bin" }] }))).toThrow("embedded model resources");
});

test("stage selection ignores superseded downloads, reuses previews and releases every temporary URL", async () => {
  const source = glb(document), requests: { url: string; signal: AbortSignal; resolve(response: Response): void }[] = [];
  const created: Blob[] = [], revoked: string[] = [];
  const preview = new FlatModelPreview({
    fetch: ((url: string, options: RequestInit) => new Promise<Response>(resolve => {
      requests.push({ url, signal: options.signal!, resolve });
    })) as typeof fetch,
    createUrl: blob => { created.push(blob); return `blob:preview-${created.length}`; },
    revokeUrl: url => revoked.push(url),
  });
  const first = preview.select("https://assets.invalid/mesh.glb", source.byteLength);
  const second = preview.select("https://assets.invalid/rig.glb", source.byteLength);
  expect(requests[0].signal.aborted).toBe(true);
  requests[1].resolve(new Response(source));
  expect(await second).toBe("blob:preview-1");
  requests[0].resolve(new Response(source));
  expect(await first).toBeNull();
  expect(await preview.select("https://assets.invalid/rig.glb", source.byteLength)).toBe("blob:preview-1");
  expect(created).toHaveLength(1);
  expect(requests.map(request => request.url)).toEqual(["https://assets.invalid/mesh.glb", "https://assets.invalid/rig.glb"]);
  const pending = preview.select("https://assets.invalid/assembly.glb", source.byteLength);
  preview.dispose();
  expect(requests[2].signal.aborted).toBe(true);
  requests[2].resolve(new Response(source));
  expect(await pending).toBeNull();
  expect(revoked).toEqual(["blob:preview-1"]);
  expect(await preview.select("https://assets.invalid/rig.glb", source.byteLength)).toBeNull();
});
