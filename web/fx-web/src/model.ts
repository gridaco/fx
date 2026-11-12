import type { Artifact } from "./index";

/** The preview reads recorded bytes only; it never resolves a model's sidecar resources. */
export const MODEL_BYTE_LIMIT = 128 * 1024 * 1024;
const object = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
// Keep this aligned with the pinned renderer, and fail rather than misrepresent required features.
const requiredExtensions = new Set([
  "KHR_draco_mesh_compression", "EXT_meshopt_compression", "KHR_mesh_quantization",
  "KHR_texture_basisu", "EXT_texture_webp", "KHR_texture_transform", "KHR_materials_unlit",
  "KHR_materials_clearcoat", "KHR_materials_ior", "KHR_materials_specular",
  "KHR_materials_transmission", "KHR_materials_volume", "KHR_materials_sheen",
  "KHR_materials_iridescence", "KHR_materials_anisotropy", "KHR_materials_emissive_strength",
  "KHR_materials_variants", "KHR_lights_punctual",
]);

/** Validate the container and embedded-resource boundary before handing bytes to a renderer. */
export function validateModel(bytes: ArrayBuffer): void {
  const invalid = () => new Error("The file is not a valid GLB 2 model.");
  if (bytes.byteLength < 20 || bytes.byteLength > MODEL_BYTE_LIMIT) throw invalid();
  const header = new DataView(bytes);
  if (header.getUint32(0, true) !== 0x46546c67 || header.getUint32(4, true) !== 2 || header.getUint32(8, true) !== bytes.byteLength) throw invalid();
  const jsonLength = header.getUint32(12, true);
  if (header.getUint32(16, true) !== 0x4e4f534a || !jsonLength || jsonLength % 4 || 20 + jsonLength > bytes.byteLength) throw invalid();
  let chunkIndex = 1;
  for (let offset = 20 + jsonLength; offset < bytes.byteLength;) {
    if (offset + 8 > bytes.byteLength) throw invalid();
    const length = header.getUint32(offset, true), type = header.getUint32(offset + 4, true);
    // Unknown chunks are ignored by the renderer. JSON is first and unique; BIN, if
    // present, is second and unique, even when custom chunks follow it.
    if (length % 4 || offset + 8 + length > bytes.byteLength || type === 0x4e4f534a || (type === 0x004e4942 && chunkIndex !== 1)) throw invalid();
    offset += 8 + length;
    chunkIndex++;
  }
  let document: unknown;
  try { document = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(bytes, 20, jsonLength))); } catch { throw invalid(); }
  if (!object(document) || !object(document.asset) || document.asset.version !== "2.0") throw invalid();
  if (document.extensionsRequired !== undefined && (!Array.isArray(document.extensionsRequired) || document.extensionsRequired.some((name) => typeof name !== "string" || !requiredExtensions.has(name)))) {
    throw new Error("This model requires an unsupported glTF extension.");
  }
  // Examine every URI, including extension resources. Unknown optional extensions must not
  // become a way to fetch a local path, a relative sidecar, or a remote texture.
  const pending: unknown[] = [document];
  while (pending.length) {
    const value = pending.pop();
    if (Array.isArray(value)) { for (const child of value) pending.push(child); }
    else if (object(value)) {
      for (const [key, child] of Object.entries(value)) {
        if (key === "uri" && (typeof child !== "string" || !/^data:(?:application\/(?:octet-stream|gltf-buffer)|image\/(?:png|jpeg|webp|ktx2));base64,[A-Za-z0-9+/=\s]*$/.test(child))) {
          throw new Error("This preview requires embedded resources. External model resources are not loaded.");
        }
        pending.push(child);
      }
    }
  }
  if (Array.isArray(document.images) && document.images.some((image) => !object(image) || (image.mimeType !== undefined && !["image/png", "image/jpeg", "image/webp", "image/ktx2"].includes(String(image.mimeType))))) {
    throw new Error("This model contains an unsupported texture format.");
  }
  const meshes = Array.isArray(document.meshes) ? document.meshes : [];
  const nodes = Array.isArray(document.nodes) ? document.nodes : [];
  const scenes = Array.isArray(document.scenes) ? document.scenes : [];
  const scene = scenes[typeof document.scene === "number" ? document.scene : 0];
  const visiting: unknown[] = object(scene) && Array.isArray(scene.nodes) ? [...scene.nodes] : [];
  const visited = new Set<number>();
  let visibleMesh = false;
  while (visiting.length) {
    const index = visiting.pop();
    if (typeof index !== "number" || !Number.isSafeInteger(index) || index < 0 || index >= nodes.length || visited.has(index)) throw invalid();
    visited.add(index);
    const node = nodes[index];
    if (!object(node)) throw invalid();
    const mesh = typeof node.mesh === "number" ? meshes[node.mesh] : undefined;
    if (object(mesh) && Array.isArray(mesh.primitives) && mesh.primitives.some((primitive) => object(primitive) && object(primitive.attributes) && Number.isInteger(primitive.attributes.POSITION))) visibleMesh = true;
    if (Array.isArray(node.children)) visiting.push(...node.children);
  }
  if (!visibleMesh) {
    throw new Error("This file has no mesh to inspect. Standalone motion and skeleton previews are not supported.");
  }
}

/** A bounded, abortable read. The URL comes from the viewer's recorded artifact inventory. */
export async function readModel(artifact: Artifact, signal: AbortSignal, fetcher: typeof fetch = fetch): Promise<ArrayBuffer> {
  if (!artifact.available || artifact.url === null) throw new Error("The recorded model file is unavailable.");
  if (!Number.isSafeInteger(artifact.size) || artifact.size < 20) throw new Error("The file is not a valid GLB 2 model.");
  if (artifact.size > MODEL_BYTE_LIMIT) throw new Error("3D previews support files up to 128 MiB. Download the original to inspect it elsewhere.");
  const response = await fetcher(artifact.url, { signal, redirect: "error" });
  if (!response.ok || !response.body) throw new Error("The recorded model file could not be read.");
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > artifact.size || size > MODEL_BYTE_LIMIT) throw new Error("The model differs from its recorded size.");
      chunks.push(value);
    }
  } catch (error) { await reader.cancel().catch(() => {}); throw error; }
  finally { reader.releaseLock(); }
  if (size !== artifact.size) throw new Error("The model differs from its recorded size.");
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  const digest = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), (part) => part.toString(16).padStart(2, "0")).join("");
  if (digest !== artifact.digest) throw new Error("The model differs from its recorded digest.");
  validateModel(bytes.buffer);
  return bytes.buffer;
}

export interface ModelSourceState { phase: "idle" | "loading" | "ready" | "error"; url: string | null; error: string | null }
interface ModelSourceIo {
  read(artifact: Artifact, signal: AbortSignal): Promise<ArrayBuffer>;
  createUrl(bytes: ArrayBuffer): string;
  revokeUrl(url: string): void;
}

/** Selection lifetime independent of React, polling, and the renderer's asynchronous callbacks. */
export class ModelSourceController {
  private state: ModelSourceState = { phase: "idle", url: null, error: null };
  private readonly listeners = new Set<() => void>();
  private request?: AbortController;
  private revision = 0;
  constructor(private readonly io: ModelSourceIo = {
    read: readModel,
    createUrl: (bytes) => URL.createObjectURL(new Blob([bytes], { type: "model/gltf-binary" })),
    revokeUrl: (url) => URL.revokeObjectURL(url),
  }) {}
  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(state: ModelSourceState) { this.state = state; this.listeners.forEach((listener) => listener()); }
  async load(artifact: Artifact): Promise<void> {
    this.dispose();
    const revision = this.revision;
    const request = new AbortController(); this.request = request;
    this.update({ phase: "loading", url: null, error: null });
    try {
      const bytes = await this.io.read(artifact, request.signal);
      if (request.signal.aborted || revision !== this.revision) return;
      this.update({ phase: "ready", url: this.io.createUrl(bytes), error: null });
    } catch (error) {
      if (request.signal.aborted || revision !== this.revision) return;
      this.update({ phase: "error", url: null, error: error instanceof Error ? error.message : "The model could not be loaded." });
    }
  }
  dispose(): void {
    this.revision++; this.request?.abort(); this.request = undefined;
    if (this.state.url) this.io.revokeUrl(this.state.url);
    this.state = { phase: "idle", url: null, error: null };
  }
}
