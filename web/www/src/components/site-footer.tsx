import { ArrowLeft } from "lucide-react";
import { site } from "../../site.config";

const resources = [
  { label: "Docs", href: site.guide_url },
  { label: "GitHub", href: site.repository_url },
  { label: "Community", href: site.community_url },
  { label: "Contact", href: site.contact_url },
];

/** A small FX-owned selection of stable Grida destinations. */
export function SiteFooter() {
  return <footer className="mt-auto px-6 pb-10">
    <div aria-hidden="true" className="mx-auto mb-7 h-px w-full max-w-6xl bg-linear-to-r from-transparent via-border to-transparent" />
    <div className="mx-auto max-w-md space-y-4 text-center">
      <nav aria-label="Footer resources" className="flex flex-wrap items-center justify-center gap-x-5 gap-y-2 text-xs text-muted-foreground">
        <a href={site.home_url} className="inline-flex items-center gap-1 font-medium text-foreground hover:text-muted-foreground">
          <ArrowLeft size={14} aria-hidden="true" /> Back
        </a>
        {resources.map(({ label, href }) => <a key={label} href={href} className="hover:text-foreground">{label}</a>)}
      </nav>
      <nav aria-label="Legal" className="flex flex-wrap items-center justify-center gap-x-2 gap-y-2 text-[11px] text-muted-foreground">
        <a href={site.privacy_url} className="hover:text-foreground">Privacy</a>
        <span aria-hidden="true">·</span>
        <a href={site.terms_url} className="hover:text-foreground">Terms</a>
      </nav>
    </div>
  </footer>;
}
