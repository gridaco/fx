import { join } from "node:path";
import { tmpdir } from "node:os";
import { safePath, type FileRef } from "../../src/lib/showcases/contracts";
import { deliveryRoot, jsonBytes, plainWrite as writeFile, sha256, wwwRoot } from "./io";
import { verifyShowcases } from "./verify";

function publicBase(value: string): string {
  const url = new URL(value);
  if (url.protocol !== "https:" || url.username || url.password || url.search || url.hash) throw new Error("Publication requires an explicit HTTPS delivery base without credentials.");
  return url.href.replace(/\/+$/, "");
}
/** A path in the delivery base is the bucket folder the closure is published under. */
export function bucketFolder(base: string): string {
  const folder = new URL(publicBase(base)).pathname.replace(/^\/+|\/+$/g, "");
  return folder && `${safePath(folder)}/`;
}
async function command(args: string[]): Promise<void> {
  // CLI auth stays in its existing credential store; never inspect it or run inside a project dotenv scope.
  const child = Bun.spawn(["tigris", ...args], { cwd: tmpdir(), stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [code, output, error] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  if (code) throw new Error(`Tigris failed (${code}): ${(error || output).slice(0, 1000)}`);
}
export async function verifyRemoteFile(base: string, file: FileRef, origin: string, fetcher: typeof fetch = fetch): Promise<void> {
  const url = `${publicBase(base)}/${safePath(file.path)}`;
  const response = await fetcher(url, { redirect: "error", headers: { Origin: origin } });
  if (!response.ok) throw new Error(`Public delivery failed: ${file.path} (HTTP ${response.status}).`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.byteLength !== file.bytes || sha256(bytes) !== file.sha256) throw new Error(`Uploaded bytes differ: ${file.path}.`);
  if (response.headers.get("content-type")?.split(";")[0].trim() !== file.media_type) throw new Error(`Wrong content type: ${file.path}.`);
  const cors = response.headers.get("access-control-allow-origin");
  if (cors !== "*" && cors !== origin) throw new Error(`Missing public-read CORS: ${file.path}.`);
  const head = await fetcher(url, { method: "HEAD", redirect: "error" });
  if (!head.ok || Number(head.headers.get("content-length")) !== file.bytes) throw new Error(`HEAD delivery failed: ${file.path}.`);
  if (file.media_type.startsWith("video/")) {
    const range = await fetcher(url, { redirect: "error", headers: { Range: "bytes=0-15", Origin: origin } });
    const rangeBytes = new Uint8Array(await range.arrayBuffer()), rangeCors = range.headers.get("access-control-allow-origin");
    if (range.status !== 206 || range.headers.get("content-range") !== `bytes 0-15/${file.bytes}` || rangeBytes.byteLength !== 16 || rangeBytes.some((value, index) => value !== bytes[index]) || (rangeCors !== "*" && rangeCors !== origin)) throw new Error(`Video range delivery failed: ${file.path}.`);
  }
}

/** Upload only a verified explicit closure. Existing different bytes are never replaced. */
export async function publishTigris(bucket: string, base: string, options: { delivery?: string; origin?: string; configureCors?: boolean } = {}) {
  if (!/^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/.test(bucket)) throw new Error("Invalid bucket name.");
  const root = options.delivery ?? deliveryRoot, origin = options.origin ?? "https://grida.co";
  const folder = bucketFolder(base);
  const verified = await verifyShowcases({ delivery: root, requireRecordings: true });
  if (options.configureCors) await command(["buckets", "set-cors", bucket, "--origins", "*", "--methods", "GET,HEAD", "--headers", "Range", "--expose-headers", "Content-Length,Content-Range,Accept-Ranges,ETag", "--override", "--json"]);
  const manifests = verified.files.filter(file => file.path.endsWith("/manifest.json"));
  const payload = verified.files.filter(file => !file.path.endsWith("/manifest.json"));
  const ordered = [...payload, ...manifests.filter(file => file.path !== `showcases/${verified.release}/manifest.json`), ...manifests.filter(file => file.path === `showcases/${verified.release}/manifest.json`)];
  let uploaded = 0, reused = 0;
  for (const file of ordered) {
    const url = `${publicBase(base)}/${file.path}`;
    const existing = await fetch(url, { method: "HEAD", redirect: "error" });
    if (existing.ok) { await verifyRemoteFile(base, file, origin); reused++; continue; }
    if (existing.status !== 404) throw new Error(`Could not establish immutable destination state: ${file.path} (HTTP ${existing.status}).`);
    await command(["objects", "put", bucket, folder + file.path, join(root, file.path), "--access", "public", "--content-type", file.media_type, "--cache-control", "public, max-age=31536000, immutable", "--json"]);
    await verifyRemoteFile(base, file, origin); uploaded++;
    console.log(`Verified ${uploaded + reused}/${ordered.length}: ${file.path}`);
  }
  const receipt = { kind: "fx-showcase-delivery-v1", delivery_base: publicBase(base), lock_sha256: verified.lock_sha256, release: verified.release, uploaded, reused, files: verified.files };
  await writeFile(join(wwwRoot, ".showcases/delivery-receipt.json"), jsonBytes(receipt));
  return receipt;
}
if (import.meta.main) {
  const [bucket, base, ...args] = process.argv.slice(2);
  if (!bucket || !base || args.some(arg => arg !== "--configure-cors")) throw new Error("Usage: publish:tigris BUCKET HTTPS_DELIVERY_BASE [--configure-cors]");
  const receipt = await publishTigris(bucket, base, { configureCors: args.includes("--configure-cors") });
  console.log(JSON.stringify({ ...receipt, files: receipt.files.length }, null, 2));
}
