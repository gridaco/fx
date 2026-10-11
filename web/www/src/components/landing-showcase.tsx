"use client";

import { useEffect, useRef, type CSSProperties } from "react";
import { Pause, Play } from "lucide-react";
import { site } from "../../site.config";
import world from "../../public/landing/examples/world/manifest.json";
import { LandingShowcaseController } from "./landing-showcase-controller";
import styles from "./landing-showcase.module.css";

const art = `${site.base_path}/landing/examples`;
const examples = ["Worlds", "Materials", "Campaigns"] as const;

/** Curated Grida home examples; React mounts independently owned interaction. */
export function LandingShowcase() {
  const root = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = new LandingShowcaseController(root.current);
    return () => controller.dispose();
  }, []);

  return <section ref={root} className={styles.showcase} aria-label="Explore what a workflow can make" data-example="worlds" data-playing="false" data-paused="true">
    <div className={styles.toolbar}>
      <div className={styles.tabs} role="tablist" aria-label="Workflow examples">
        {examples.map((name, index) => {
          const id = name.toLowerCase();
          return <button key={id} type="button" role="tab" id={`example-tab-${id}`} aria-controls={`example-panel-${id}`} aria-selected={index === 0} tabIndex={index === 0 ? 0 : -1} data-showcase-tab={id}>{name}</button>;
        })}
      </div>
      <button className={styles.playback} type="button" data-showcase-playback aria-label="Play preview">
        <Pause size={14} className={styles.pauseIcon} aria-hidden="true" />
        <Play size={14} className={styles.playIcon} aria-hidden="true" />
      </button>
    </div>

    <div role="tabpanel" id="example-panel-worlds" aria-labelledby="example-tab-worlds" data-showcase-panel="worlds" tabIndex={0}>
      <div className={styles.composition}>
        <figure className={styles.source}>
          <div className={styles.layerStack}>
            <img src={world.reference} width={1280} height={720} alt="Prepared landscape layers composed into a sunlit garden" draggable={false} />
          </div>
          <figcaption>Your layers</figcaption>
        </figure>
        <Connection />
        <figure className={styles.result}>
          <div className={styles.world} role="img" aria-label="Five landscape layers scrolling at different speeds to form a seamless world">
            {world.layers.map(layer => <div key={layer.id} className={styles.worldLayer} style={{ backgroundImage: `url(${layer.file})`, animationDuration: `${36 / layer.parallax}s`, zIndex: layer.order } as CSSProperties} />)}
          </div>
          <figcaption>A world that keeps going.</figcaption>
        </figure>
      </div>
    </div>

    <div role="tabpanel" id="example-panel-materials" aria-labelledby="example-tab-materials" data-showcase-panel="materials" tabIndex={0} hidden>
      <div className={styles.composition}>
        <figure className={styles.source}>
          <div className={styles.sketch}>
            <svg viewBox="0 0 200 190" role="img" aria-label="Line sketch of a curved cantilever chair">
              <g fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
                <path d="M 76 20 Q 72 17 70 25 L 60 86 Q 60 98 75 102 L 126 113 Q 147 118 143 130 L 132 155 Q 130 160 119 160 L 60 149 Q 51 147 55 142 L 65 135" />
                <path d="M 76 20 L 131 32 Q 139 34 137 44 L 127 98 Q 125 103 132 107 L 155 114 Q 174 120 168 133 L 153 166 Q 150 172 139 170 L 60 153" />
                <path d="M 60 86 L 127 101 M 75 102 L 103 89 M 126 113 L 151 104 M 119 160 L 144 146 L 79 134 L 55 142" />
                <path d="M 73 30 L 66 81 M 133 42 L 126 86 M 153 123 L 141 150" opacity=".3" />
              </g>
            </svg>
          </div>
          <figcaption>Your sketch</figcaption>
        </figure>
        <Connection />
        <figure className={styles.result}>
          <div className={styles.materials}>
            {["Natural oak", "Brushed aluminum", "Smoked resin"].map((name, index) => <div key={name} className={styles.material}>
              <img src={`${art}/materials.webp`} width={1536} height={1024} loading="lazy" alt={`The same chair in ${name.toLowerCase()}`} style={{ left: `${-index * 100}%` }} draggable={false} />
              <span>{name}</span>
            </div>)}
          </div>
          <figcaption>One form. New possibilities.</figcaption>
        </figure>
      </div>
    </div>

    <div role="tabpanel" id="example-panel-campaigns" aria-labelledby="example-tab-campaigns" data-showcase-panel="campaigns" tabIndex={0} hidden>
      <div className={styles.composition}>
        <figure className={styles.source}>
          <div className={styles.product}>
            <img src={`${art}/campaign/headphones.webp`} width={900} height={900} loading="lazy" alt="Silver headphones with charcoal cushions" draggable={false} />
          </div>
          <figcaption>Your product</figcaption>
        </figure>
        <Connection />
        <figure className={styles.result}>
          <div className={styles.campaign}>
            <img className={styles.poster} src={`${art}/campaign/poster-v1.webp`} width={800} height={1200} loading="lazy" alt="Headphone campaign poster: Less noise. More you." draggable={false} />
            <img className={styles.social} src={`${art}/campaign/social-v1.webp`} width={900} height={900} loading="lazy" alt="Matching social creative: Find your off switch." draggable={false} />
            <video className={styles.film} data-src={`${art}/campaign/motion-v2.mp4`} poster={`${art}/campaign/motion-v2-poster.webp`} muted loop playsInline preload="none" disablePictureInPicture aria-label="Matching campaign film of silver headphones in a blue studio" />
          </div>
          <figcaption>One product. A whole campaign.</figcaption>
        </figure>
      </div>
    </div>
  </section>;
}

function Connection() {
  return <div className={styles.connection} aria-hidden="true"><i /><span className="fx-function-mark">f<span>(</span>x<span>)</span></span><i /></div>;
}
