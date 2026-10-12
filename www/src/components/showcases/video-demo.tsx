"use client";

import { useEffect, useRef } from "react";
import type { VideoDemoSlug } from "../../lib/showcases/contracts";
import { VideoDemoController } from "./video-demo-controller";
import styles from "./video-demo.module.css";

export function VideoDemo({ slug, assetBase }: { slug: VideoDemoSlug; assetBase: string }) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = new VideoDemoController(root.current, slug, assetBase);
    return () => controller.dispose();
  }, [slug, assetBase]);
  return <div ref={root} className={styles.demo}>
    <div data-video-content />
    <p data-video-status role="status">Loading recorded video example…</p>
  </div>;
}
