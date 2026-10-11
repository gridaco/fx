import Link from "next/link";
import { ArrowRight } from "lucide-react";
import { site } from "../../site.config";
import { GettingStarted } from "../components/getting-started";
import { LandingShowcase } from "../components/landing-showcase";
import { LandingMotion } from "../components/landing-motion";
import { fx_description, pageMetadata } from "../lib/metadata";

export const metadata = pageMetadata("Grida FX — Grida", fx_description);

const software = {
  "@context": "https://schema.org",
  "@type": "SoftwareSourceCode",
  name: "Grida FX",
  description: fx_description,
  url: site.url,
  codeRepository: site.repository_url,
  programmingLanguage: ["Rust", "Python", "TypeScript"],
  license: "https://www.apache.org/licenses/LICENSE-2.0",
  author: { "@type": "Organization", name: "Grida", url: site.origin },
  isPartOf: { "@type": "WebSite", name: "Grida", url: site.origin },
};

export default function HomePage() {
  return <LandingMotion className="fx-site-container flex flex-1 flex-col pt-10 pb-12 sm:pt-12">
    <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: JSON.stringify(software).replace(/</g, "\\u003c") }} />
    <div className="grid grid-cols-1 gap-8">
      <section className="text-left" aria-labelledby="hero-title">
        <p className="sr-only">Grida FX</p>
        <h1 id="hero-title" className="text-5xl leading-[1.05] font-semibold tracking-[-0.05em] text-balance sm:text-[3.5rem]">Go with the flow.</h1>
        <p className="mt-4 max-w-md text-base leading-7 text-balance text-muted-foreground">Reusable workflows for images, audio, and more.</p>
      </section>
      <LandingShowcase />
      <section id="start" aria-label="Explore or install Grida FX" className="grid w-full max-w-sm gap-5">
        <Link href="/workflows" className="fx-landing-cta inline-flex h-11 items-center justify-self-start gap-2 rounded-full bg-primary px-5 text-sm font-medium text-white hover:bg-primary/90">
          Explore workflows <ArrowRight size={16} aria-hidden="true" />
        </Link>
        <GettingStarted />
      </section>
    </div>
  </LandingMotion>;
}
