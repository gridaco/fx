"use client";

import { MoveHorizontal } from "lucide-react";
import { useEffect, useRef, type CSSProperties } from "react";
import { comparisonPosition, ImageCompareController, wrapComparisonGeometry, type WrapComparison } from "./image-compare-controller";
import styles from "./image-compare.module.css";

export type ComparisonImage = { src: string; alt: string; label: string };

type ImageCompareProps = {
  before: ComparisonImage;
  after: ComparisonImage;
  /** Both images use this registered canvas, including any transparent padding. */
  width: number;
  height: number;
  initial?: number;
  caption?: string;
  wrap?: WrapComparison;
};

/** React supplies the document; the controller owns the comparison interaction. */
export function ImageCompare({ before, after, width, height, initial = 50, caption, wrap }: ImageCompareProps) {
  const root = useRef<HTMLElement>(null);
  const position = comparisonPosition(initial);
  useEffect(() => {
    if (!root.current) return;
    const controller = new ImageCompareController(root.current, position);
    return () => controller.dispose();
  }, [position, before.label, after.label]);

  const geometry = wrap && wrapComparisonGeometry(width, height, wrap);
  const style = {
    "--compare-position": `${position}%`,
    "--compare-ratio": wrap ? wrap.width / wrap.height : width / height,
    ...(geometry && {
      "--compare-source-width": `${geometry.imageWidth}%`,
      "--compare-source-height": `${geometry.imageHeight}%`,
      "--compare-source-top": `${geometry.imageTop}%`,
      "--compare-repaint-width": `${geometry.repaintWidth}%`,
    }),
  } as CSSProperties;

  return <figure ref={root} className={styles.root} style={style}>
    <div className={styles.stage}>
      <ImageLayer image={after} width={width} height={height} wrap={!!wrap} />
      <ImageLayer image={before} width={width} height={height} wrap={!!wrap} before />
      {wrap && <>
        <div className={styles.repaintBand} aria-hidden="true" />
        <div className={styles.join} data-wrap-join aria-hidden="true" />
      </>}
      <div className={styles.labels} aria-hidden="true">
        <span>{before.label}</span><span>{after.label}</span>
      </div>
      <div className={styles.divider} aria-hidden="true" />
      <div className={styles.handle} aria-hidden="true"><MoveHorizontal size={18} strokeWidth={1.75} /></div>
      <input
        className={styles.range}
        data-compare-range
        data-before-label={before.label}
        data-after-label={after.label}
        type="range"
        min={0}
        max={100}
        step={1}
        defaultValue={position}
        aria-label={`Compare ${before.label} and ${after.label}`}
        aria-valuetext={`${position}% ${before.label}, ${100 - position}% ${after.label}`}
      />
    </div>
    {caption && <figcaption className={styles.caption}>{caption}</figcaption>}
  </figure>;
}

function ImageLayer({ image, width, height, wrap, before }: { image: ComparisonImage; width: number; height: number; wrap: boolean; before?: boolean }) {
  return <div className={`${styles.layer} ${before ? styles.before : ""}`} role={wrap ? "img" : undefined} aria-label={wrap ? image.alt : undefined}>
    {wrap ? <>
      <img className={`${styles.wrapped} ${styles.tail}`} src={image.src} alt="" width={width} height={height} loading="lazy" decoding="async" />
      <img className={`${styles.wrapped} ${styles.head}`} src={image.src} alt="" width={width} height={height} loading="lazy" decoding="async" />
    </> : <img src={image.src} alt={image.alt} width={width} height={height} loading="lazy" decoding="async" />}
  </div>;
}
