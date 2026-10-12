import { object } from "../../lib/showcases/contracts";

const unlit = "KHR_materials_unlit";

/** A temporary presentation copy. Geometry, images, skinning and animation bytes stay intact. */
export function flatModelBytes(source: ArrayBuffer): Uint8Array<ArrayBuffer> {
  const header = new DataView(source);
  if (source.byteLength < 20 || header.getUint32(0, true) !== 0x46546c67 || header.getUint32(4, true) !== 2
    || header.getUint32(8, true) !== source.byteLength || header.getUint32(16, true) !== 0x4e4f534a) {
    throw new Error("The character preview requires a valid GLB 2 model.");
  }
  const jsonLength = header.getUint32(12, true), end = 20 + jsonLength;
  if (jsonLength % 4 || end > source.byteLength) throw new Error("The character GLB has an invalid JSON chunk.");
  for (let offset = end; offset < source.byteLength;) {
    if (offset + 8 > source.byteLength) throw new Error("The character GLB has an incomplete chunk.");
    const size = header.getUint32(offset, true);
    if (size % 4 || offset + 8 + size > source.byteLength) throw new Error("The character GLB has an invalid chunk length.");
    offset += 8 + size;
  }
  const document = object(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(source, 20, jsonLength))));
  if (!Array.isArray(document.materials) || !document.materials.length) throw new Error("The character GLB has no preview materials.");
  // Blob URLs cannot resolve a model's relative resources. These showcase GLBs embed all media.
  for (const key of ["buffers", "images"]) {
    const resources = document[key];
    if (!Array.isArray(resources)) continue;
    for (const resource of resources) {
      const { uri } = object(resource);
      if (uri !== undefined && (typeof uri !== "string" || !uri.startsWith("data:"))) {
        throw new Error("The flat character preview requires embedded model resources.");
      }
    }
  }
  for (const item of document.materials) {
    const material = object(item);
    material.extensions = { ...object(material.extensions ?? {}), [unlit]: {} };
  }
  for (const key of ["extensionsUsed", "extensionsRequired"]) {
    const extensions = document[key] ?? [];
    if (!Array.isArray(extensions) || extensions.some(item => typeof item !== "string")) throw new Error("Invalid character GLB extensions.");
    document[key] = [...new Set([...extensions, unlit])];
  }
  const json = new TextEncoder().encode(JSON.stringify(document)), paddedLength = Math.ceil(json.byteLength / 4) * 4;
  const output = new Uint8Array(20 + paddedLength + source.byteLength - end), view = new DataView(output.buffer);
  output.set(new Uint8Array(source, 0, 12));
  view.setUint32(8, output.byteLength, true);
  view.setUint32(12, paddedLength, true); view.setUint32(16, 0x4e4f534a, true);
  output.fill(0x20, 20, 20 + paddedLength); output.set(json, 20);
  output.set(new Uint8Array(source, end), 20 + paddedLength);
  return output;
}

type PreviewIo = { fetch(url: string, options: RequestInit): Promise<Response>; createUrl(blob: Blob): string; revokeUrl(url: string): void };

/** Own the temporary URLs and pending selection independently of the DOM or model-viewer internals. */
export class FlatModelPreview {
  private readonly urls = new Map<string, string>();
  private pending?: AbortController;
  private revision = 0;
  private disposed = false;

  constructor(private readonly io: PreviewIo = {
    fetch: (...args) => fetch(...args), createUrl: blob => URL.createObjectURL(blob), revokeUrl: url => URL.revokeObjectURL(url),
  }) {}

  async select(source: string, bytes: number): Promise<string | null> {
    if (this.disposed) return null;
    this.pending?.abort();
    const revision = ++this.revision;
    const cached = this.urls.get(source);
    if (cached) return cached;
    const request = new AbortController(); this.pending = request;
    try {
      const response = await this.io.fetch(source, { signal: request.signal, cache: "force-cache", redirect: "error" });
      if (!response.ok) throw new Error(`The character model could not load (HTTP ${response.status}).`);
      const sourceBytes = await response.arrayBuffer();
      if (request.signal.aborted || revision !== this.revision) return null;
      if (sourceBytes.byteLength !== bytes) throw new Error("The character model differs from its published size.");
      const url = this.io.createUrl(new Blob([flatModelBytes(sourceBytes)], { type: "model/gltf-binary" }));
      this.urls.set(source, url);
      return url;
    } catch (error) {
      if (request.signal.aborted || revision !== this.revision) return null;
      throw error;
    } finally {
      if (this.pending === request) this.pending = undefined;
    }
  }

  dispose(): void {
    this.disposed = true; this.revision++; this.pending?.abort();
    for (const url of this.urls.values()) this.io.revokeUrl(url);
    this.urls.clear();
  }
}
