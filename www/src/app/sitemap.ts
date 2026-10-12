import type { MetadataRoute } from "next";
import { pageUrl } from "../lib/metadata";
import { workflows } from "../lib/workflows";

export const dynamic = "force-static";

/** Every indexable page, from the catalog that generates them; the build checks it against the export. */
export default function sitemap(): MetadataRoute.Sitemap {
  return [
    { url: pageUrl() },
    { url: pageUrl("workflows") },
    ...workflows.map(({ slug }) => ({ url: pageUrl(`workflows/${slug}`) })),
  ];
}
