import { expect, test } from "bun:test";
import { mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { checkSitemap, exportedPageUrl } from "./sitemap";

const page = (canonical: string) => `<html><head><link rel="canonical" href="${canonical}"/><meta name="robots" content="index, follow"/></head></html>`;
const notFound = `<html><head><meta name="robots" content="noindex"/><meta name="robots" content="noindex, nofollow"/></head></html>`;
const sitemap = (...urls: string[]) => `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls.map(url => `<url>\n<loc>${url}</loc>\n</url>\n`).join("")}</urlset>\n`;
const site = ["https://grida.co/fx", "https://grida.co/fx/workflows", "https://grida.co/fx/workflows/acme"];

async function exported(files: Record<string, string>, check: (output: string) => Promise<unknown>): Promise<void> {
  const output = await mkdtemp(join(await realpath(tmpdir()), "fx-sitemap-"));
  try {
    for (const [path, text] of Object.entries(files)) { await mkdir(dirname(join(output, path)), { recursive: true }); await writeFile(join(output, path), text); }
    await check(output);
  } finally { await rm(output, { recursive: true, force: true }); }
}
const pages = { "index.html": page(site[0]), "workflows.html": page(site[1]), "workflows/acme.html": page(site[2]), "404.html": notFound };

test("exported files map to their served URLs on grida.co without trailing slashes", () => {
  expect(["index.html", "workflows.html", "workflows/acme.html"].map(exportedPageUrl)).toEqual(site);
});
test("a sitemap listing every indexable page at its canonical URL passes", async () => {
  await exported({ ...pages, "sitemap.xml": sitemap(...site) }, async output => expect(await checkSitemap(output)).toBe(3));
});
test("a page left out, a stray or duplicate URL, a noindex page and a mismatched canonical fail", async () => {
  const cases: [Record<string, string>, string][] = [
    [{ ...pages, "workflows/img-a.html": page("https://grida.co/fx/workflows/img-a"), "sitemap.xml": sitemap(...site) }, "img-a is missing"],
    [{ ...pages, "sitemap.xml": sitemap(...site, "https://grida-fx.vercel.app/fx/workflows/acme") }, "grida-fx.vercel.app/fx/workflows/acme is not an indexable"],
    [{ ...pages, "sitemap.xml": sitemap(...site, "https://grida.co/fx/workflows/") }, "workflows/ is not an indexable"],
    [{ ...pages, "sitemap.xml": sitemap(...site, site[2]) }, "listed twice"],
    [{ ...pages, "sitemap.xml": sitemap(...site, "https://grida.co/fx/404") }, "noindex page is listed"],
    [{ ...pages, "workflows.html": page("https://grida.co/fx/workflows/"), "sitemap.xml": sitemap(...site) }, "canonical is https://grida.co/fx/workflows/, not"],
  ];
  for (const [files, message] of cases) await exported(files, async output => { await expect(checkSitemap(output)).rejects.toThrow(message); });
});
