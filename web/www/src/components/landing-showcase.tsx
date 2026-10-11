"use client";

import { useEffect, useRef, type CSSProperties } from "react";
import { Pause, Play } from "lucide-react";
import world from "../../public/landing/examples/world/manifest.json";
import { LandingShowcaseController } from "./landing-showcase-controller";
import styles from "./landing-showcase.module.css";

/** One curated landscape example; React mounts independently owned playback. */
export function LandingShowcase() {
  const root = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = new LandingShowcaseController(root.current);
    return () => controller.dispose();
  }, []);

  return <section ref={root} className={styles.showcase} aria-label="From landscape layers to a looping world" data-playing="false" data-paused="true">
    <div className={styles.composition}>
      <figure className={styles.source}>
        <div className={styles.layerStack}>
          <img src={world.reference} width={1280} height={720} alt="Prepared landscape layers composed into a sunlit garden" draggable={false} />
        </div>
        <figcaption>Your layers</figcaption>
      </figure>
      <div className={styles.connection} aria-hidden="true"><i /><span className="fx-function-mark">f<span>(</span>x<span>)</span></span><i /></div>
      <figure className={styles.result}>
        <div className={styles.worldFrame}>
          <div className={styles.world} role="img" aria-label="Five landscape layers scrolling at different speeds to form a seamless world">
            {world.layers.map(layer => <div key={layer.id} className={styles.worldLayer} style={{ backgroundImage: `url(${layer.file})`, animationDuration: `${36 / layer.parallax}s`, zIndex: layer.order } as CSSProperties} />)}
          </div>
          <button className={styles.playback} type="button" data-showcase-playback aria-label="Play preview">
            <Pause size={14} className={styles.pauseIcon} aria-hidden="true" />
            <Play size={14} className={styles.playIcon} aria-hidden="true" />
          </button>
        </div>
        <figcaption>A world that keeps going.</figcaption>
      </figure>
    </div>
  </section>;
}
