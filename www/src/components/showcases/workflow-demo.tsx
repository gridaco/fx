import type { Workflow } from "../../lib/workflows";
import { MotionDemo } from "./motion-demo";
import { WorldModelDemo } from "./world-model-demo";
import { TerrainUiDemo } from "./terrain-ui-demo";
import { PetDemo } from "./pet-demo";
import type { ParallaxGuideLayer } from "./parallax-guides";
import { VideoDemo } from "./video-demo";

export function WorkflowDemo({ slug, assetBase, parallaxGuides }: { slug: Workflow["slug"]; assetBase: string; parallaxGuides?: readonly ParallaxGuideLayer[] }) {
  if (slug === "product-promo" || slug === "character-performance") return <VideoDemo slug={slug} assetBase={assetBase} />;
  if (slug === "parallax" || slug === "movie-sprite" || slug === "portrait-motion") return <MotionDemo slug={slug} assetBase={assetBase} parallaxGuides={parallaxGuides} />;
  if (slug === "world" || slug === "character-3d") return <WorldModelDemo slug={slug} assetBase={assetBase} />;
  if (slug === "pet") return <PetDemo assetBase={assetBase} />;
  return <TerrainUiDemo slug={slug} assetBase={assetBase} />;
}
