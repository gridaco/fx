"use client";

import { useEffect, useRef } from "react";
import { MovieDemoController, ParallaxDemoController, PortraitDemoController } from "./motion-controller";
import styles from "./motion-demo.module.css";

type MotionSlug = "parallax" | "movie-sprite" | "portrait-motion";

/** React supplies the document; independently owned classes handle playback. */
export function MotionDemo({ slug, assetBase }: { slug: MotionSlug; assetBase: string }) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const Controller = slug === "parallax" ? ParallaxDemoController : slug === "movie-sprite" ? MovieDemoController : PortraitDemoController;
    const controller = new Controller(root.current, assetBase);
    return () => controller.dispose();
  }, [slug, assetBase]);

  const asset = (path: string) => `${assetBase.replace(/\/$/, "")}/${path}`;
  return <div ref={root} className={`fx-demo ${styles.demo}`}>
    {slug === "parallax" ? <>
      <div className={styles.parallaxStage} data-stage>
        <canvas width={1536} height={1024} tabIndex={0} aria-label="Three-layer meadow. Drag horizontally or use the left and right arrow keys to pan." />
      </div>
      <div className={styles.toolbar}>
        <button type="button" data-play disabled>Pause</button>
        <button type="button" data-reset disabled>Reset</button>
        <label className={styles.inline}>Speed <input data-speed type="range" min={0} max={400} step={10} defaultValue={100} /><output data-speed-value className={styles.numeric}>100 px/s</output></label>
        <div className={styles.layerControls} aria-label="Visible layers">
          {["far", "middle", "foreground"].map(name => <label key={name} className={styles.inline}><input type="checkbox" defaultChecked disabled data-layer={name} />{name[0].toUpperCase() + name.slice(1)}</label>)}
        </div>
      </div>
    </> : slug === "movie-sprite" ? <>
      <div className={styles.movieStages} data-movie-stages data-comparing="false">
        <figure>
          <div className={styles.figureStage} data-stage data-background="checker">
            <video data-movie src={asset("assets/loop.webm")} poster={asset("assets/canonical.png")} muted loop playsInline preload="metadata" aria-label="Finished transparent idle loop" />
            <img data-fallback src={asset("assets/loop.webp")} loading="lazy" alt="Transparent animated idle loop" hidden />
          </div>
          <figcaption>Finished loop <span>Face pinned · Transparent</span></figcaption>
        </figure>
        <figure data-original-stage hidden>
          <div className={styles.figureStage}>
            <video data-original src={asset("assets/original-unpinned-preview.mp4")} muted loop playsInline preload="metadata" aria-label="Original generated take before facial finishing" />
          </div>
          <figcaption>Original take <span>Before face pinning</span></figcaption>
        </figure>
      </div>
      <div className={styles.toolbar}>
        <PlaybackControls />
        <label className={styles.inline}><input type="checkbox" data-compare />Compare original take</label>
        <select data-format defaultValue="video" aria-label="Playback format"><option value="video">WebM</option><option value="image">Animated WebP</option></select>
        <BackgroundControls />
      </div>
      <Timeline />
    </> : <>
      <div className={styles.portraitLayout}>
        <div className={styles.figureStage} data-stage data-background="checker">
          <canvas data-portrait width={720} height={1280} aria-label="Character with separately composited body, eyes and mouth" />
          <img data-fallback src={asset("assets/animation.webp")} loading="lazy" alt="Authored facial animation" hidden />
        </div>
        <div className={styles.portraitControls}>
          <div className={styles.zoomStage}><canvas data-face-zoom width={231} height={231} aria-label="Face detail at its native size" /></div>
          <output data-selection className={styles.selection}>Open eyes · Rest</output>
          <div className={styles.segment} aria-label="Animation layers">
            <button type="button" data-mode="combined" aria-pressed="true">Body + face</button>
            <button type="button" data-mode="body" aria-pressed="false">Body</button>
            <button type="button" data-mode="face" aria-pressed="false">Face</button>
          </div>
          <label className={styles.inline}><input type="checkbox" data-automatic defaultChecked />Play authored expressions</label>
          <div className={styles.expressionControls}>
            <label>Eyes<select data-eyes disabled defaultValue="rest"><option value="rest">Open</option><option value="eyes_half">Half blink</option><option value="eyes_closed">Closed</option></select></label>
            <label>Mouth<select data-mouth disabled defaultValue="rest"><option value="rest">Rest</option><option value="mouth_smile">Smile</option><option value="mouth_a">Open A</option></select></label>
          </div>
          <p className={styles.hint}>Eyes and mouth are independent patches. The original transparency stays intact.</p>
        </div>
      </div>
      <video data-body src={asset("assets/body.webm")} crossOrigin="anonymous" muted loop playsInline preload="metadata" hidden />
      <div className={styles.toolbar}><PlaybackControls /><BackgroundControls /></div>
      <Timeline />
    </>}
    <p className={styles.status} data-status role="status">Loading preview…</p>
  </div>;
}

function PlaybackControls() {
  return <>
    <button type="button" data-play>Play</button>
    <button type="button" data-reset>Restart</button>
    <select data-speed defaultValue="1" aria-label="Playback speed"><option value=".5">0.5×</option><option value="1">1×</option><option value="1.5">1.5×</option></select>
  </>;
}

function BackgroundControls() {
  return <div className={styles.backgrounds} aria-label="Preview background">
    <button type="button" data-background="checker" aria-pressed="true">Checker</button>
    <button type="button" data-background="paper" aria-pressed="false">Paper</button>
    <button type="button" data-background="slate" aria-pressed="false">Slate</button>
  </div>;
}

function Timeline() {
  return <label className={styles.timeline}><span className="sr-only">Playback position</span><input data-scrub type="range" min={0} max={8} step={.01} defaultValue={0} /><output data-time className={styles.numeric}>0.00 / 8.00 s</output></label>;
}
