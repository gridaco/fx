import lockDocument from "../../../content/showcases/lock.json";
import { site } from "../../../site.config";
import { parseLock, safePath, type ShowcaseSlug } from "./contracts";

export const showcaseLock = parseLock(lockDocument);
/** A trusted deployment root; object identities and paths never contain a provider hostname. */
export function deliveryBase(): string {
  const base = process.env.FX_SHOWCASE_ASSET_BASE ?? site.base_path;
  if (base.startsWith("/")) {
    if (base.startsWith("//") || /[?#\\]/.test(base)) throw new Error("Invalid showcase delivery base.");
    return base.replace(/\/+$/, "");
  }
  const url = new URL(base);
  if (!["https:", "http:"].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new Error("Invalid showcase delivery base.");
  return url.href.replace(/\/+$/, "");
}
export function deliveryUrl(path: string): string { return `${deliveryBase()}/${safePath(path)}`; }
export function workflowAssetPath(slug: string, file: string): string {
  if (!(slug in showcaseLock.workflows)) throw new Error(`No published showcase for ${slug}.`);
  const relative = `showcases/${showcaseLock.release.sha256}/${slug}`;
  return `${deliveryUrl(relative)}${file ? `/${safePath(file)}` : "/"}`;
}
export function workflowDemoManifest(slug: ShowcaseSlug): { url: string; sha256: string; contract: string } {
  const item = showcaseLock.workflows[slug].demo;
  return { url: deliveryUrl(`showcases/${showcaseLock.release.sha256}/${item.manifest.path}`), sha256: item.manifest.sha256, contract: item.contract };
}
export function workflowRecordings(slug: string): readonly { id: string; label: string; url: string; sha256: string }[] {
  return (showcaseLock.workflows[slug]?.recordings ?? []).map(item => ({ id: item.id, label: item.label, url: deliveryUrl(item.path), sha256: item.sha256 }));
}
export function metadataPin(slug: ShowcaseSlug, path: string): { sha256: string; bytes: number } {
  const demo = showcaseLock.workflows[slug].demo;
  const found = [demo.manifest, ...demo.metadata].find(item => item.path === `${slug}/${safePath(path)}`);
  if (!found) throw new Error(`Unpinned showcase metadata: ${slug}/${path}.`);
  return found;
}
