"use client";

import { useEffect, useRef } from "react";
import { Pause, Play, StepForward } from "lucide-react";
import { PetDemoController } from "./pet-controller";
import styles from "./pet-demo.module.css";

/** React lays out the page; the class owns playback, selection and the sheet marker. */
export function PetDemo({ assetBase }: { assetBase: string }) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = new PetDemoController(root.current, assetBase);
    return () => controller.dispose();
  }, [assetBase]);

  return <div ref={root} className={`fx-demo ${styles.demo}`}>
    <div className={styles.layout}>
      <div className={styles.sheetArea}>
        <div className={styles.labels} data-states role="group" aria-label="States" />
        <div className={styles.sheet} data-sheet>
          <img src={`${assetBase.replace(/\/+$/, "")}/assets/spritesheet.webp`} crossOrigin="anonymous" width={1536} height={1872} alt="The whole sprite sheet: one row of frames per state" />
          <span className={styles.band} aria-hidden="true" />
          <span className={styles.cell} aria-hidden="true" />
        </div>
      </div>
      <div className={styles.player}>
        {/* The panel fills its half; the frame stays at its native 192 x 208 and never scales up. */}
        <div className={styles.stage} data-stage data-background="checker">
          <canvas data-pet width={384} height={416} tabIndex={0} aria-label="The selected state at its recorded frame timings. Left and right arrow keys step through its frames." />
        </div>
        <p className={styles.caption}><output data-selection>Loading…</output><span data-frame className={styles.numeric} /></p>
        <div className={styles.toolbar}>
          <button type="button" data-play disabled aria-label="Pause" title="Pause" className={styles.icon}>
            <span data-icon="pause"><Pause size={14} aria-hidden="true" /></span><span data-icon="play" hidden><Play size={14} aria-hidden="true" /></span>
          </button>
          <button type="button" data-step disabled aria-label="Next frame" title="Next frame" className={styles.icon}><StepForward size={14} aria-hidden="true" /></button>
          <select data-speed defaultValue="1" disabled aria-label="Playback speed"><option value=".5">0.5×</option><option value="1">1×</option><option value="2">2×</option></select>
        </div>
        <div className={styles.toolbar} aria-label="Preview background">
          <button type="button" data-background="checker" aria-pressed="true">Checker</button>
          <button type="button" data-background="paper" aria-pressed="false">Paper</button>
          <button type="button" data-background="slate" aria-pressed="false">Slate</button>
        </div>
      </div>
    </div>
    <p className={styles.status} data-status role="status">Loading sprite sheet…</p>
  </div>;
}
