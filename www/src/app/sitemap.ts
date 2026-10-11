import type { MetadataRoute } from "next";
import { site } from "../../site.config";
import { workflows } from "../lib/workflows";

export const dynamic = "force-static";

export default function sitemap(): MetadataRoute.Sitemap {
  return [
    { url: site.url },
    { url: `${site.url}/workflows` },
    ...workflows.map(({ slug }) => ({ url: `${site.url}/workflows/${slug}` })),
  ];
}
