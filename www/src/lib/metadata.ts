import type { Metadata } from "next";
import { site } from "../../site.config";

export const fx_description = "Grida FX connects models, local tools, and checks into reusable asset workflows. Plan, run, inspect, and reuse every step.";

/** A page's canonical URL on grida.co, without a trailing slash. The sitemap lists the same URLs. */
export function pageUrl(path = ""): string {
  return path ? `${site.url}/${path}` : site.url;
}

/** Emit complete social metadata per page: Next replaces nested metadata objects. */
export function pageMetadata(title: string, description: string, path = ""): Metadata {
  const url = pageUrl(path);
  const images = [{
    url: `${site.url}/social-preview.png`,
    width: 1200,
    height: 630,
    alt: "Grida FX — Go with the flow. Asset workflows, as code, with an illustrated mountain landscape.",
  }];
  return {
    title,
    description,
    alternates: { canonical: url },
    openGraph: { title, description, url, siteName: "Grida", locale: "en_US", type: "website", images },
    twitter: { card: "summary_large_image", title, description, site: "@grida_co", creator: "@grida_co", images },
  };
}
