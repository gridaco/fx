"use client";

import Link from "next/link";
import { useSelectedLayoutSegment } from "next/navigation";
import { site } from "../../site.config";

/** Grida's logo geometry, with navigation owned by FX. */
export function SiteNav() {
  const segment = useSelectedLayoutSegment();

  return <header className="absolute top-0 right-0 left-0 z-50">
    <nav aria-label="Main navigation" className="fx-site-container py-4 xl:py-8">
      <div className="grid h-9 grid-cols-[1fr_auto_1fr] grid-rows-1 items-center gap-3 lg:gap-8">
        <a href={site.home_url} className="flex items-center justify-center justify-self-start rounded-sm" aria-label="Grida home">
          <img src={`${site.base_path}/brand/grida-mark.svg`} width={20} height={20} className="block size-5" alt="" />
        </a>
        <div className="flex items-center rounded-full bg-muted/95 p-1 shadow-[0_2px_8px_rgb(0_0_0/0.035)]">
          <Link href="/" aria-current={segment === null ? "page" : undefined}
            className="rounded-full px-4 py-2 text-sm font-medium text-zinc-600 hover:text-foreground aria-[current=page]:bg-white aria-[current=page]:text-foreground aria-[current=page]:shadow-sm sm:px-6">
            About
          </Link>
          <Link href="/workflows/" aria-current={segment === "workflows" ? "page" : undefined}
            className="rounded-full px-4 py-2 text-sm font-medium text-zinc-600 hover:text-foreground aria-[current=page]:bg-white aria-[current=page]:text-foreground aria-[current=page]:shadow-sm sm:px-6">
            Workflows
          </Link>
        </div>
        <a href={site.guide_url} className="flex h-9 items-center justify-self-end rounded-sm text-sm text-muted-foreground hover:text-foreground">Docs</a>
      </div>
    </nav>
  </header>;
}
