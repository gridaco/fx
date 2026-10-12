import Link from "next/link";
import { ArrowRight } from "lucide-react";
import { site } from "../../site.config";
import { GettingStarted } from "../components/getting-started";
import { LandingReel } from "../components/landing-reel";
import { LandingMotion } from "../components/landing-motion";
import { fx_description, pageMetadata } from "../lib/metadata";
import { coverRecording, workflowCover } from "../lib/showcases/covers";
import { workflows } from "../lib/workflows";

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

const reel = workflows.map(workflow => ({
  slug: workflow.slug,
  title: workflow.title,
  media: workflowCover(workflow.slug),
  recording: coverRecording(workflow.slug),
}));

export default function HomePage() {
  return <LandingMotion className="fx-site-container flex flex-1 flex-col pt-10 pb-12 sm:pt-12">
    <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: JSON.stringify(software).replace(/</g, "\\u003c") }} />
    <div className="grid grid-cols-1 gap-8 md:gap-10">
      <section aria-labelledby="hero-title">
        <p className="sr-only">Grida FX</p>
        <h1 id="hero-title" className="text-5xl leading-[1.05] font-semibold tracking-[-0.05em] text-balance sm:text-[3.5rem]">Go with the flow.</h1>
        <p className="mt-4 max-w-md text-base leading-7 text-balance text-muted-foreground">Reusable workflows for images, audio, and more.</p>
      </section>
      <div className="grid grid-cols-1 gap-16 md:gap-24">
        <LandingReel items={reel} />
        {/* The reel's grid, mirrored: both calls to action stacked in the 3 of a 3:7 row, never narrower than the command. */}
        <section id="start" aria-label="Get started" className="grid grid-cols-1 sm:grid-cols-[minmax(20rem,3fr)_minmax(0,7fr)] sm:gap-4">
          <div className="grid gap-3">
            <Link href="/workflows" className="inline-flex h-11 items-center justify-between gap-2 rounded-full bg-primary pr-4 pl-5 text-sm font-medium text-white transition-colors hover:bg-primary/90 motion-reduce:transition-none">
              Explore workflows <ArrowRight size={16} aria-hidden="true" />
            </Link>
            <GettingStarted />
          </div>
        </section>
      </div>
    </div>
  </LandingMotion>;
}
