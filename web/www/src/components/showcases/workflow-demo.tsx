import type { Workflow } from "../../lib/workflows";
import { MotionDemo } from "./motion-demo";
import { WorldModelDemo } from "./world-model-demo";
import { TerrainUiDemo } from "./terrain-ui-demo";

export function WorkflowDemo({ slug, assetBase }: { slug: Workflow["slug"]; assetBase: string }) {
  if (slug === "parallax" || slug === "movie-sprite" || slug === "portrait-motion") return <MotionDemo slug={slug} assetBase={assetBase} />;
  if (slug === "world" || slug === "character-3d") return <WorldModelDemo slug={slug} assetBase={assetBase} />;
  return <TerrainUiDemo slug={slug} assetBase={assetBase} />;
}
