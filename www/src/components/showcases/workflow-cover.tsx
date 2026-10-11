"use client";

import { useEffect, useRef } from "react";
import type { WorkflowCoverMedia } from "../../lib/showcases/covers";
import { WorkflowCoverController } from "./workflow-cover-controller";

export function WorkflowCover({ media, title, eager = false }: { media: WorkflowCoverMedia; title: string; eager?: boolean }) {
  const video = useRef<HTMLVideoElement>(null);
  const src = media.kind === "video" ? media.src : undefined;
  useEffect(() => {
    if (!video.current || !src) return;
    const controller = new WorkflowCoverController(video.current, src);
    return () => controller.dispose();
  }, [src]);

  const className = `size-full ${media.fit === "contain" ? "object-contain" : "object-cover"}`;
  const style = { objectPosition: media.position ?? "center" };
  const image = <img src={media.src} alt={title} width={960} height={600} loading={eager ? "eager" : "lazy"} decoding="async" className={className} style={style} />;
  if (media.kind === "image") return image;
  if (media.kind === "animated_image") return <picture className="block size-full">
    <source media="(prefers-reduced-motion: reduce)" srcSet={media.poster} />
    {image}
  </picture>;
  return <video ref={video} poster={media.poster} muted loop playsInline preload="none" width={960} height={600} className={className} style={style} aria-hidden="true" />;
}
