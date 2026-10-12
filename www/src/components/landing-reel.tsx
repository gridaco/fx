"use client";

import { useEffect, useRef } from "react";
import Link from "next/link";
import { ArrowRight } from "lucide-react";
import type { WorkflowCoverMedia } from "../lib/showcases/covers";
import "@grida/fx-web/canvas.css";
import { LandingReelController } from "./landing-reel-controller";
import styles from "./landing-reel.module.css";

/** `recording` is the run the result came from; the workflow card draws it. */
export type ReelItem = {
  slug: string;
  title: string;
  media: WorkflowCoverMedia;
  recording: { url: string; sha256: string; focus: string | null } | null;
};

/**
 * Two floating cards per workflow: the result it made, large, and its recorded graph overlapping
 * it. Static markup for every result; the controller decides which workflow is shown.
 */
export function LandingReel({ items }: { items: readonly ReelItem[] }) {
  const root = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = new LandingReelController(root.current);
    return () => controller.dispose();
  }, []);

  return <section ref={root} className={styles.reel} aria-roledescription="carousel" aria-label="Workflows and the results they made" data-state="paused">
    <div className={styles.stage}>
      <div className={styles.previewCard}>
        {items.map((item, index) => <Link key={item.slug} href={`/workflows/${item.slug}`} data-reel-item
          data-active={index === 0 ? "true" : undefined} aria-hidden={index !== 0} tabIndex={-1} className={styles.item}
          data-recording-url={item.recording?.url} data-recording-sha256={item.recording?.sha256} data-recording-focus={item.recording?.focus ?? undefined}>
          <Media media={item.media} title={item.title} eager={index === 0} />
        </Link>)}
      </div>
      {/* The workflow: the showcase pages' canvas over the verified run, never interactive here. */}
      <div className={styles.workflowCard}>
        {[0, 1].map(layer => <div key={layer} data-reel-layer className={styles.layer} aria-hidden="true">
          <div data-reel-canvas className={styles.canvas} />
        </div>)}
        {items.map((item, index) => <Link key={item.slug} href={`/workflows/${item.slug}#graph`} data-reel-caption data-index={index}
          hidden={index !== 0} className={styles.cardLink} aria-label={`See the ${item.title} workflow`} />)}
      </div>
    </div>
    <div className={styles.bar}>
      {items.map((item, index) => <Link key={item.slug} href={`/workflows/${item.slug}`} data-reel-caption data-index={index} hidden={index !== 0}
        className="inline-flex min-w-0 items-center gap-1.5 text-sm font-medium">
        <span className="truncate">{item.title}</span> <ArrowRight size={14} aria-hidden="true" className="shrink-0" />
      </Link>)}
      <div className={styles.controls}>
        {items.map((item, index) => <button key={item.slug} type="button" data-reel-segment className={styles.segment}
          data-active={index === 0 ? "true" : undefined} aria-current={index === 0} aria-label={`Show ${item.title}`}>
          <span className={styles.track}><span data-reel-fill className={styles.fill} /></span>
        </button>)}
      </div>
    </div>
  </section>;
}

function Media({ media, title, eager }: { media: WorkflowCoverMedia; title: string; eager: boolean }) {
  const className = `size-full ${media.fit === "contain" ? "object-contain" : "object-cover"}`;
  const style = { objectPosition: media.position ?? "center" };
  const loading = eager ? "eager" : "lazy";
  if (media.kind === "video") return <video data-src={media.src} poster={media.poster} muted loop playsInline preload="none"
    width={960} height={600} className={className} style={style} aria-label={title} />;
  const image = <img src={media.src} alt={title} width={960} height={600} loading={loading} decoding="async" className={className} style={style} />;
  if (media.kind === "image") return image;
  return <picture className="block size-full">
    <source media="(prefers-reduced-motion: reduce)" srcSet={media.poster} />
    {image}
  </picture>;
}
