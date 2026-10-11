import { createHash } from "node:crypto";
import { lstat, mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { safePath, type FileRef } from "../../src/lib/showcases/contracts";

export const wwwRoot = fileURLToPath(new URL("../../", import.meta.url));
export const contentRoot = join(wwwRoot, "content/showcases");
export const deliveryRoot = join(wwwRoot, ".showcases/delivery");
export const sha256 = (value: Uint8Array | string) => createHash("sha256").update(value).digest("hex");
export const jsonBytes = (value: unknown) => Buffer.from(`${JSON.stringify(value, null, 2)}\n`);

export async function ensureDirectory(path: string, _options?: { recursive?: boolean }): Promise<void> {
  const ancestors: string[] = [];
  for (let item = resolve(path); dirname(item) !== item; item = dirname(item)) ancestors.unshift(item);
  for (const ancestor of ancestors) {
    await mkdir(ancestor).catch((error: NodeJS.ErrnoException) => { if (error.code !== "EEXIST") throw error; });
    const info = await lstat(ancestor);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error("Showcase destination ancestors must be real directories.");
  }
}
export async function plainWrite(path: string, bytes: Uint8Array): Promise<void> {
  await ensureDirectory(dirname(path));
  const existing = await lstat(path).catch((error: NodeJS.ErrnoException) => { if (error.code === "ENOENT") return null; throw error; });
  if (existing && (!existing.isFile() || existing.isSymbolicLink())) throw new Error("Showcase destination files must be regular files.");
  await writeFile(path, bytes);
}

/** Refuse symlink ancestors, including when a caller supplies the source root. */
function buildPath(relative: string): string {
  if (!relative || /[\\\u0000-\u001f\u007f]/.test(relative) || relative.split("/").some(part => !part || part === "." || part === "..") || /^[A-Za-z]:/.test(relative)) throw new Error("Build output requires confined relative filesystem paths.");
  return relative;
}
async function readConfined(root: string, relative: string): Promise<Buffer> {
  const absoluteRoot = resolve(root);
  const ancestors: string[] = [];
  for (let path = absoluteRoot; dirname(path) !== path; path = dirname(path)) ancestors.unshift(path);
  for (const path of ancestors) {
    const info = await lstat(path);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error("Showcase roots require real directories.");
  }
  const parts = relative.split("/");
  for (let index = 0; index < parts.length; index++) {
    const info = await lstat(join(root, ...parts.slice(0, index + 1)));
    if (info.isSymbolicLink() || (index === parts.length - 1 ? !info.isFile() : !info.isDirectory())) throw new Error(`Unsafe showcase file: ${relative}.`);
  }
  return readFile(join(root, relative));
}
export async function confinedFile(root: string, relative: string): Promise<Buffer> {
  return readConfined(root, safePath(relative));
}
/** Next owns filesystem names such as [slug] and $d$slug; these are never public asset references. */
export async function confinedBuildFile(root: string, relative: string): Promise<Buffer> {
  return readConfined(root, buildPath(relative));
}
export async function files(root: string, prefix = ""): Promise<string[]> {
  return walkFiles(root, prefix, safePath);
}
export async function buildFiles(root: string): Promise<string[]> {
  return walkFiles(root, "", buildPath);
}
async function walkFiles(root: string, prefix: string, validate: (path: string) => string): Promise<string[]> {
  const result: string[] = [];
  for (const entry of await readdir(join(root, prefix), { withFileTypes: true })) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name; validate(relative);
    if (entry.isSymbolicLink()) throw new Error("Showcases cannot contain symlinks.");
    if (entry.isDirectory()) result.push(...await walkFiles(root, relative, validate));
    else if (entry.isFile()) result.push(relative);
    else throw new Error("Showcases require ordinary files.");
  }
  return result.sort();
}
export function mediaType(path: string): string {
  const extension = path.split(".").at(-1)!;
  const type = ({ png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", webp: "image/webp", gif: "image/gif", mp4: "video/mp4", webm: "video/webm", glb: "model/gltf-binary", json: "application/json", js: "text/javascript", wasm: "application/wasm", md: "text/markdown" } as Record<string, string>)[extension];
  if (type) return type;
  if (/LICENSE(?:$|\.)/.test(path)) return "text/plain";
  throw new Error(`No approved media type for ${path}.`);
}
export function fileRef(path: string, bytes: Uint8Array): FileRef {
  return { path: safePath(path), sha256: sha256(bytes), bytes: bytes.byteLength, media_type: mediaType(path) };
}
export async function verifiedFile(root: string, file: FileRef): Promise<Buffer> {
  const bytes = await confinedFile(root, file.path);
  if (bytes.byteLength !== file.bytes || sha256(bytes) !== file.sha256) throw new Error(`Content changed: ${file.path}.`);
  return bytes;
}
