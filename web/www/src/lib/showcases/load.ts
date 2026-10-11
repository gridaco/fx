import { metadataPin } from "./catalog";
import { safePath, type ShowcaseSlug } from "./contracts";

/** Every dynamic demo document is checked against the bytes selected for this website build. */
export async function loadShowcaseJson<T>(slug: ShowcaseSlug, base: string, path: string, parse: (value: unknown) => T, signal?: AbortSignal): Promise<T> {
  const pin = metadataPin(slug, path);
  const response = await fetch(`${base.replace(/\/+$/, "")}/${safePath(path)}`, { signal, cache: "force-cache", redirect: "error" });
  if (!response.ok) throw new Error(`This showcase could not load (HTTP ${response.status}).`);
  const bytes = await response.arrayBuffer();
  const hash = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), part => part.toString(16).padStart(2, "0")).join("");
  if (bytes.byteLength !== pin.bytes || hash !== pin.sha256) throw new Error("This showcase failed its integrity check. Its content differs from this website's published version.");
  let value: unknown;
  try { value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)); } catch { throw new Error("This showcase document is not valid UTF-8 JSON."); }
  return parse(value);
}
