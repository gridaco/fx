"use client";

import { createElement, useEffect, useRef } from "react";
import { CharacterModelController, WorldGalleryController } from "./world-model-controller";

/** React owns the page structure; the controllers own the interactive exhibit. */
export function WorldModelDemo({ slug, assetBase }: { slug: "world" | "character-3d"; assetBase: string }) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = slug === "world"
      ? new WorldGalleryController(root.current, assetBase)
      : new CharacterModelController(root.current, assetBase);
    return () => controller.dispose();
  }, [slug, assetBase]);

  return <div ref={root} className="fx-demo">
    {slug === "world" ? <>
      <div data-world-nav className="grid grid-cols-3 gap-3 sm:grid-cols-6" aria-label="Explore the world" />
      <div className="mt-7 grid gap-7 lg:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)]">
        <figure className="min-w-0">
          <div className="flex aspect-[4/3] items-center justify-center overflow-hidden rounded-lg bg-muted">
            {/* Real run media comes from the separately hosted artifact bundle. */}
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img data-world-image alt="" className="h-full w-full object-contain" />
          </div>
          <figcaption data-world-lesson className="mt-3 text-sm leading-6 text-muted-foreground" />
        </figure>
        <div className="min-w-0">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p data-world-kind className="text-xs text-muted-foreground" />
            <span data-world-verdict className="rounded-full border px-2.5 py-1 text-xs" />
          </div>
          <h2 data-world-name aria-live="polite" className="mt-3 text-2xl font-medium tracking-tight" />
          <p data-world-summary className="mt-3 text-sm leading-7 text-muted-foreground" />
          <p data-world-life className="mt-4 text-sm leading-7" />
          <div className="mt-5 border-t pt-5">
            <h3 className="text-sm font-medium">The tension</h3>
            <p data-world-tension className="mt-2 text-sm leading-6 text-muted-foreground" />
          </div>
          <div data-world-relations className="mt-5 flex flex-wrap gap-2" aria-label="Related entities" />
        </div>
      </div>
      <details className="mt-7 border-y py-4 text-sm">
        <summary className="cursor-pointer font-medium">Story, scene & image review</summary>
        <div className="mt-5 grid gap-7 text-sm leading-7 lg:grid-cols-2">
          <div><h3 className="font-medium">The scene</h3><p data-world-scene className="mt-2 text-muted-foreground" /><h3 className="mt-5 font-medium">Established facts</h3><ul data-world-facts className="mt-2 list-disc space-y-2 pl-5 text-muted-foreground" /></div>
          <div><h3 className="font-medium">Independent image review</h3><p data-world-review className="mt-2 text-muted-foreground" /><ul data-world-findings className="mt-3 list-disc space-y-2 pl-5 text-muted-foreground" /></div>
        </div>
      </details>
      <p data-demo-status role="status" className="mt-3 text-xs text-muted-foreground">Loading the world…</p>
    </> : <>
      <div data-model-stages className="flex flex-wrap gap-2" role="group" aria-label="From reference to rig" />
      <div className="relative mt-4 flex h-[min(65vh,600px)] min-h-80 items-center justify-center overflow-hidden rounded-lg bg-muted">
        {createElement("model-viewer", {
          "data-character-model": "",
          alt: "Mira Vale, the meadow caretaker, as a textured 3D character",
          "camera-controls": "",
          "interaction-prompt": "none",
          "shadow-intensity": "0.5",
          "tone-mapping": "neutral",
          exposure: "1",
          "camera-orbit": "25deg 78deg auto",
          "touch-action": "pan-y",
          className: "block h-full w-full",
        })}
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img data-model-reference alt="Mira Vale character reference" hidden className="h-full w-full object-contain p-5" />
        <span data-model-loading className="pointer-events-none absolute bottom-4 rounded-full bg-background/90 px-3 py-1.5 text-xs text-muted-foreground">Loading the 3D viewer…</span>
      </div>
      <div className="mt-4 flex flex-wrap items-center justify-between gap-4">
        <div data-model-clips className="flex flex-wrap gap-2" role="group" aria-label="Diagnostic animation clips" />
        <div className="flex flex-wrap items-center gap-4 text-xs">
          <button type="button" data-model-pause disabled>Pause</button>
          <button type="button" data-model-reset>Reset camera</button>
          <label className="flex cursor-pointer items-center gap-2"><input data-model-rotate type="checkbox" /> Rotate</label>
        </div>
      </div>
      <p data-model-caption className="mt-4 text-xs leading-6 text-muted-foreground" />
      <details className="mt-7 border-y py-4 text-sm">
        <summary className="cursor-pointer font-medium">Reference views & rig evidence</summary>
        <div data-model-images className="mt-5 grid grid-cols-3 gap-3" />
        <div className="mt-6">
          <label className="text-xs text-muted-foreground" htmlFor="character-evidence">Pose inspection</label>
          <select data-model-evidence id="character-evidence" className="ml-3 max-w-full rounded-md border bg-background px-3 py-2 text-xs" />
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img data-model-evidence-image alt="Rig inspection rendered from five views" className="mt-4 w-full rounded-md bg-muted object-contain" loading="lazy" />
        </div>
      </details>
      <p data-demo-status role="status" className="mt-3 text-xs text-muted-foreground">Loading the recorded character…</p>
    </>}
  </div>;
}
