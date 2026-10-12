import { workflowAssetPath } from "./showcases/catalog";

export const workflows = [
  {
    slug: "parallax",
    title: "Parallax background",
    description: "Separate a concept image into repeating layers, with scroll factors and an optional looping preview.",
    input: "Concept image",
    output: "Three scrolling layers",
    note: "The middle layer uses mirrored extension, so some landmarks repeat.",
  },
  {
    slug: "world",
    title: "Illustrated storyworld",
    description: "Expand a synopsis and reference image into structured entities and individually reviewed illustrations.",
    input: "World concept",
    output: "Six illustrated entities",
    note: "Four images passed the workflow's image judge. Two remain studies; wind-instrument geometry varies between illustrations.",
  },
  {
    slug: "movie-sprite",
    title: "Transparent sprite loop",
    description: "Animate a character illustration, remove the background, and finish a transparent idle loop.",
    input: "Character illustration",
    output: "Transparent idle loop",
    note: "A four-second body-motion loop with generated blinks. Trimming, keying and loop closure are local; no face hold is applied.",
  },
  {
    slug: "portrait-motion",
    title: "Facial animation patches",
    description: "Generate independent eye and mouth states as reusable patches, aligned to the original character image.",
    input: "High-resolution face image",
    output: "Six reusable face patches",
    note: "The timeline is authored for demonstration. No speech audio or lip synchronization is included.",
  },
  {
    slug: "character-3d",
    title: "Rigged 3D character",
    description: "Generate reference views, build and rig a textured mesh, and export a GLB with diagnostic animation clips.",
    input: "Character brief and reference",
    output: "Skinned GLB and six clips",
    note: "The recorded rig includes a local lower-body weight correction. Game and runtime qualification are still pending.",
  },
  {
    slug: "terrain",
    title: "47-tile terrain atlas",
    description: "Generate a 47-tile autotile atlas, with a neighborhood lookup and a local map preview.",
    input: "Landscape direction",
    output: "47-tile terrain atlas",
    note: "This is block terrain with repeated texture motifs. Arbitrary layouts have not been exhaustively reviewed.",
  },
  {
    slug: "ui-kit",
    title: "Nine-slice UI kit",
    description: "Generate a coordinated nine-slice panel, four button states, and a sixteen-icon sheet.",
    input: "Visual direction",
    output: "Nine-slice interface kit",
    note: "The panel and buttons passed the workflow's judge. The icons are retained as a study because their palette differs from the requested direction.",
  },
] as const;

export type Workflow = (typeof workflows)[number];

export function workflowAsset(workflow: Workflow, file: string): string {
  return workflowAssetPath(workflow.slug, file);
}
