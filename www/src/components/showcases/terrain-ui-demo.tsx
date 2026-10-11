"use client";

import { useEffect, useRef } from "react";
import { Eraser, Grid2X2, MoveDiagonal2, Pencil, RotateCcw, X } from "lucide-react";
import { TerrainDemoController, UiKitDemoController } from "./terrain-ui-controller";
import styles from "./terrain-ui-demo.module.css";

import { uiGlyphs as glyphs } from "../../lib/showcases/contracts";
const buttonStates = ["normal", "hover", "pressed", "disabled"];

/** The page owns its content; these class-controlled demonstrations only consume the run's assets. */
export function TerrainUiDemo({ slug, assetBase }: { slug: "terrain" | "ui-kit"; assetBase: string }) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = slug === "terrain" ? new TerrainDemoController(root.current, assetBase) : new UiKitDemoController(root.current, assetBase);
    return () => controller.dispose();
  }, [slug, assetBase]);

  return <div ref={root} className={`fx-demo ${styles.root}`} data-ready="false">
    {slug === "terrain" ? <Terrain base={assetBase} /> : <UiKit />}
  </div>;
}

function Terrain({ base }: { base: string }) {
  return <>
    <fieldset data-controls disabled className={styles.controls} aria-label="Terrain controls">
      <div className={styles.group} aria-label="Terrain layouts">
        <button type="button" data-preset="meadow" aria-pressed="true">Meadow</button>
        <button type="button" data-preset="islands" aria-pressed="false">Islands</button>
        <button type="button" data-preset="cavern" aria-pressed="false">Cavern</button>
      </div>
      <div className={styles.group} aria-label="Painting tools">
        <button type="button" data-tool="paint" aria-pressed="true"><Pencil size={14} aria-hidden="true" />Paint</button>
        <button type="button" data-tool="erase" aria-pressed="false"><Eraser size={14} aria-hidden="true" />Erase</button>
        <button type="button" data-reset><RotateCcw size={14} aria-hidden="true" /><span>Reset</span></button>
      </div>
      <label><input type="checkbox" data-grid /><Grid2X2 size={14} aria-hidden="true" />Grid</label>
      <label className={styles.camera}>Camera<input type="range" min="0" max="100" defaultValue="0" data-camera aria-label="Horizontal camera position" /></label>
    </fieldset>
    <canvas className={styles.terrainCanvas} data-terrain-canvas aria-label="Editable terrain. Drag to paint; use Erase or hold Shift to remove tiles." />
    <div className={styles.caption}><span>Drag to paint. Shift-drag to erase.</span><span data-status role="status">Loading atlas…</span></div>
    <details className={styles.details}>
      <summary>Inspect the 47-tile atlas</summary>
      <p className={styles.help}>Each painted cell checks its eight neighbours and selects a tile. Point at a cell to highlight its source.</p>
      <div className={styles.atlas}>
        <img src={`${base.replace(/\/+$/, "")}/assets/atlas.png`} width="1440" height="480" alt="Generated terrain atlas with 47 neighbour-aware tiles" loading="lazy" />
        <span data-atlas-highlight className={styles.atlasHighlight} hidden />
      </div>
    </details>
  </>;
}

function UiKit() {
  return <>
    <fieldset data-controls disabled className={styles.controls} aria-label="Interface controls">
      <div className={styles.group} aria-label="Panel proportions">
        <button type="button" data-size="wide" aria-pressed="true">Wide</button>
        <button type="button" data-size="compact" aria-pressed="false">Compact</button>
        <button type="button" data-size="tall" aria-pressed="false">Tall</button>
      </div>
      <label><input type="checkbox" data-disable />Disable primary action</label>
      <span className={styles.status} data-status role="status">Loading sheets…</span>
    </fieldset>
    <div className={styles.kitStage} data-kit-stage>
      <div className={styles.panel} data-panel>
        <canvas data-panel-art className={styles.panelArt} aria-hidden="true" />
        <div className={styles.panelContent}>
          <div className={styles.panelTop}><span>FIELD JOURNAL / DAY 03</span><button type="button" data-close className={styles.close} aria-label="Close journal"><X size={17} /></button></div>
          <h3>The meadow is waiting.</h3>
          <p className={styles.journalCopy}>Pack a few essentials, take the long way home, and see what the afternoon brings.</p>
          <div className={styles.inventory} aria-label="Select an item">
            {[["heart", "Kindness"], ["star", "Wonder"], ["home", "A way home"]].map(([glyph, label], index) => <button key={glyph} type="button" data-choice={glyph} aria-pressed={index === 0}>
              <canvas data-icon={glyph} aria-hidden="true" /><span>{label}</span>
            </button>)}
          </div>
          <p className={styles.selection} data-selection>Kindness packed</p>
          <div className={styles.actions}>
            <button type="button" data-primary data-painted-button className={styles.paintedButton}><canvas aria-hidden="true" /><span>Begin the walk</span></button>
            <button type="button" data-secondary data-painted-button className={styles.paintedButton}><canvas aria-hidden="true" /><span>Rest a little</span></button>
          </div>
          <p className={styles.feedback} data-feedback aria-live="polite">Your next small adventure starts here.</p>
        </div>
        <button type="button" data-resize className={styles.resize} aria-label="Resize journal; drag or use the arrow keys"><MoveDiagonal2 size={14} /></button>
      </div>
      <button type="button" data-reopen className={styles.reopen} hidden>Open journal</button>
    </div>
    <p className={styles.help}>Drag the corner to resize the frame. Text and controls stay live; the generated sheets supply the artwork.</p>
    <div className={styles.uiDetails}>
      <div>
        <h3 className={styles.detailHeading}>Sixteen icons</h3>
        <div className={styles.iconGrid} aria-label="Inspect generated icons">
          {glyphs.map(glyph => <button key={glyph} type="button" data-inspect-icon={glyph} aria-label={glyph.replaceAll("_", " ")} aria-pressed={glyph === "heart"}><canvas data-icon={glyph} aria-hidden="true" /></button>)}
        </div>
        <p className={styles.iconName} data-icon-name>heart</p>
      </div>
      <div>
        <h3 className={styles.detailHeading}>Four button states</h3>
        <div className={styles.buttonStates}>
          {buttonStates.map(state => <figure key={state}><canvas data-button-state={state} aria-label={`${state} button artwork`} /><figcaption>{state}</figcaption></figure>)}
        </div>
      </div>
    </div>
  </>;
}
