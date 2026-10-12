import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { site } from "../site.config";

const wwwRoot = fileURLToPath(new URL("../", import.meta.url));

/** Where the static export serves a page: `index.html` at the site root, `a/b.html` at `/a/b`. */
export function exportedPageUrl(file: string): string {
  const path = file.split("\\").join("/").replace(/\.html$/, "").replace(/(^|\/)index$/, "");
  return path ? `${site.url}/${path}` : site.url;
}

async function htmlFiles(root: string, folder = root): Promise<string[]> {
  const files: string[] = [];
  for (const item of await readdir(folder, { withFileTypes: true })) {
    const path = join(folder, item.name);
    if (item.isDirectory()) files.push(...await htmlFiles(root, path));
    else if (item.name.endsWith(".html")) files.push(relative(root, path));
  }
  return files.sort();
}

/** The sitemap lists exactly the indexable exported pages, each at the canonical URL the page declares. */
export async function checkSitemap(output: string): Promise<number> {
  const locs = [...(await readFile(join(output, "sitemap.xml"), "utf8")).matchAll(/<loc>([^<]*)<\/loc>/g)].map(match => match[1]);
  const listed = new Set(locs), pages = new Set<string>(), errors: string[] = [];
  if (listed.size !== locs.length) errors.push("sitemap.xml: a URL is listed twice.");
  for (const file of await htmlFiles(output)) {
    const html = await readFile(join(output, file), "utf8"), url = exportedPageUrl(file);
    const noindex = [...html.matchAll(/<meta name="robots" content="([^"]*)"/g)].some(match => /\bnoindex\b/.test(match[1]));
    if (noindex) { if (listed.has(url)) errors.push(`${file}: a noindex page is listed in sitemap.xml.`); continue; }
    const canonical = html.match(/<link rel="canonical" href="([^"]*)"/)?.[1];
    if (canonical !== url) errors.push(`${file}: its canonical is ${canonical ?? "missing"}, not ${url}.`);
    if (!listed.has(url)) errors.push(`${file}: ${url} is missing from sitemap.xml.`);
    pages.add(url);
  }
  for (const loc of listed) if (!pages.has(loc)) errors.push(`sitemap.xml: ${loc} is not an indexable exported page.`);
  if (errors.length) throw new Error(`Sitemap check failed:\n${errors.join("\n")}`);
  return pages.size;
}

if (import.meta.main) console.log(`The sitemap lists all ${await checkSitemap(join(wwwRoot, "out"))} indexable pages at their canonical URLs.`);
