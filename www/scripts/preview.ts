import { realpath, stat } from "node:fs/promises";
import { resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { site } from "../site.config";

const root = await realpath(fileURLToPath(new URL("../out/", import.meta.url))).catch(() => {
  throw new Error("Build the site first: bun --no-env-file run build:www");
});
const port = Number(process.env.PORT ?? 3100);
if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("PORT must be between 1 and 65535.");

/** Serve only the static export, mounted exactly as it will be at grida.co/fx. */
const server = Bun.serve({
  hostname: "127.0.0.1",
  port,
  async fetch(request) {
    if (!["GET", "HEAD"].includes(request.method)) return new Response(null, { status: 405, headers: { Allow: "GET, HEAD" } });
    const url = new URL(request.url);
    if (url.pathname === "/" || url.pathname === site.base_path) {
      return Response.redirect(new URL(`${site.base_path}/${url.search}`, url), 308);
    }
    let path: string;
    try { path = decodeURIComponent(url.pathname); }
    catch { return new Response("Invalid path", { status: 400 }); }
    if (!path.startsWith(`${site.base_path}/`)) return new Response("Not found", { status: 404 });
    const relative = path.slice(site.base_path.length + 1);
    if (relative.includes("\0") || relative.includes("\\") || relative.split("/").some((part) => part === ".." || part.startsWith("."))) {
      return new Response("Not found", { status: 404 });
    }
    let candidate = resolve(root, relative);
    if ((await stat(candidate).catch(() => null))?.isDirectory()) candidate = resolve(candidate, "index.html");
    const confined = await realpath(candidate).catch(() => null);
    if (!confined || !confined.startsWith(`${root}${sep}`) || !(await stat(confined)).isFile()) {
      return new Response(request.method === "HEAD" ? null : Bun.file(resolve(root, "404.html")), { status: 404, headers: { "Content-Type": "text/html; charset=utf-8" } });
    }
    const file = Bun.file(confined);
    return new Response(request.method === "HEAD" ? null : file, { headers: { "Content-Type": file.type } });
  },
});

console.log(`FX www preview: http://${server.hostname}:${server.port}${site.base_path}/`);
