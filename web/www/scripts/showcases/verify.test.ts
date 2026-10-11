import { expect, test } from "bun:test";
import { cp, mkdtemp, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { confinedBuildFile, contentRoot, plainWrite, sha256 } from "./io";
import { verifyBuildBinding, verifyShowcases, writeBuildBinding } from "./verify";
import { verifyRemoteFile } from "./publish-tigris";

test("the frozen catalog supports all active readers without large local media", async () => {
  const result = await verifyShowcases({ requireRecordings: true });
  expect(result.metadata).toBeGreaterThanOrEqual(12);
  expect(result.recordings).toBeGreaterThanOrEqual(7);
});
test("a changed frozen document fails before adapter parsing", async () => {
  const root = await mkdtemp(join(await realpath(tmpdir()), "fx-showcase-lock-"));
  try {
    await cp(contentRoot, root, { recursive: true });
    await writeFile(join(root, "manifests/parallax/manifest.json"), "{}\n");
    await expect(verifyShowcases({ content: root })).rejects.toThrow("Content changed");
  } finally { await rm(root, { recursive: true, force: true }); }
});
test("delivery verification checks bytes, MIME, CORS, HEAD and video ranges", async () => {
  const bytes = Buffer.from("0123456789abcdefghijklmnop");
  const file = { path: "showcases/fixture/loop.webm", sha256: sha256(bytes), bytes: bytes.length, media_type: "video/webm" };
  const fetcher = (async (_url: unknown, init?: RequestInit) => {
    const range = new Headers(init?.headers).has("Range");
    return new Response(init?.method === "HEAD" ? null : range ? bytes.subarray(0, 16) : bytes, {
      status: range ? 206 : 200, headers: { "content-type": "video/webm", "access-control-allow-origin": "*", "content-length": String(bytes.length), ...(range ? { "content-range": `bytes 0-15/${bytes.length}` } : {}) },
    });
  }) as typeof fetch;
  await expect(verifyRemoteFile("https://assets.example", file, "https://grida.co", fetcher)).resolves.toBeUndefined();
  await expect(verifyRemoteFile("https://assets.example", { ...file, sha256: "a".repeat(64) }, "https://grida.co", fetcher)).rejects.toThrow("bytes differ");
  const noCors = (() => Promise.resolve(new Response(bytes, { headers: { "content-type": "video/webm" } }))) as unknown as typeof fetch;
  await expect(verifyRemoteFile("https://assets.example", file, "https://grida.co", noCors)).rejects.toThrow("CORS");
  for (const invalid of ["wrong_bytes", "missing_cors"]) {
    const badRange = (async (url: unknown, init?: RequestInit) => {
      if (!new Headers(init?.headers).has("Range")) return fetcher(url as string, init);
      return new Response(invalid === "wrong_bytes" ? Buffer.alloc(16) : bytes.subarray(0, 16), { status: 206, headers: { "content-range": `bytes 0-15/${bytes.length}`, ...(invalid === "missing_cors" ? {} : { "access-control-allow-origin": "*" }) } });
    }) as typeof fetch;
    await expect(verifyRemoteFile("https://assets.example", file, "https://grida.co", badRange)).rejects.toThrow("range delivery");
  }
});
test("Next output names are hashable without weakening public asset confinement", async () => {
  const root = await mkdtemp(join(await realpath(tmpdir()), "fx-next-output-"));
  try {
    const output = join(root, "out"), path = join(root, "build.json");
    for (const filename of ["_next/static/chunks/app/workflows/[slug]/page-1eb61dee0a6a5aaa.js", "workflows/parallax/__next.workflows.$d$slug.__PAGE__.txt"]) await plainWrite(join(output, filename), Buffer.from("generated"));
    await writeBuildBinding("/fx", { output, path });
    const binding = await verifyBuildBinding("/fx", { output, path });
    expect(binding.output).toHaveLength(2);
    await expect(confinedBuildFile(output, "../build.json")).rejects.toThrow("confined");
    await symlink(path, join(output, "leak.json"));
    await expect(verifyBuildBinding("/fx", { output, path })).rejects.toThrow("symlink");
  } finally { await rm(root, { recursive: true, force: true }); }
});
test("candidate checks refuse stale output and a base different from its successful build", async () => {
  const root = await mkdtemp(join(await realpath(tmpdir()), "fx-showcase-build-"));
  try {
    const output = join(root, "out"), path = join(root, "build.json");
    await import("node:fs/promises").then(fs => fs.mkdir(output));
    await writeFile(join(output, "index.html"), "<main>Candidate A</main>");
    await writeBuildBinding("https://assets.example", { output, path });
    await expect(verifyBuildBinding("https://assets.example", { output, path })).resolves.toHaveProperty("kind", "fx-showcase-build-v1");
    await expect(verifyBuildBinding("https://other.example", { output, path })).rejects.toThrow("delivery base");
    await writeFile(join(output, "index.html"), "<main>Candidate B</main>");
    await expect(verifyBuildBinding("https://assets.example", { output, path })).rejects.toThrow("changed since");
  } finally { await rm(root, { recursive: true, force: true }); }
});
