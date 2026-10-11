import { workflowAssetPath } from "./showcases/catalog";

export const workflows = [
  {
    slug: "parallax",
    title: "A world in layers",
    description: "Turn one illustrated meadow into a scene with depth. Pan through the landscape and explore each layer.",
    input: "Concept image",
    output: "Three scrolling layers",
    note: "The middle layer uses mirrored extension, so some landmarks repeat.",
  },
  {
    slug: "world",
    title: "One idea, a whole world",
    description: "Meet the people, places, and objects of Windward Meadow. Six illustrated studies, connected by one direction.",
    input: "World concept",
    output: "Six illustrated entities",
    note: "Four images passed the workflow's image judge. Two remain studies; wind-instrument geometry varies between illustrations.",
  },
  {
    slug: "movie-sprite",
    title: "An illustration comes alive",
    description: "An eight-second character idle, ready to play on any background. Compare the original character with the finished motion.",
    input: "Character illustration",
    output: "Transparent idle loop",
    note: "The finished loop uses a local face pin to correct blinking and drift in the generated take.",
  },
  {
    slug: "portrait-motion",
    title: "Small changes, more expression",
    description: "Control a character's eyes and mouth independently. Mix nine expressions with the same body motion.",
    input: "Character and body idle",
    output: "Six reusable face patches",
    note: "The timeline is authored for demonstration. No speech audio or lip synchronization is included.",
  },
  {
    slug: "character-3d",
    title: "From drawing to dimension",
    description: "Orbit a textured character and try six diagnostic poses. Follow the illustration into a skinned 3D model.",
    input: "Character reference",
    output: "Skinned GLB and six clips",
    note: "The asset pipeline accepted this model. Game and runtime qualification are still pending.",
  },
  {
    slug: "terrain",
    title: "Paint your own path",
    description: "Build a little landscape with an illustrated terrain atlas. Paint, erase, and try different layouts.",
    input: "Landscape direction",
    output: "47-mask terrain atlas",
    note: "This is block terrain with repeated texture motifs. Arbitrary layouts have not been exhaustively reviewed.",
  },
  {
    slug: "ui-kit",
    title: "A world with its own interface",
    description: "A journal, buttons, and sixteen icons from the same visual direction. Resize the panel and try the controls.",
    input: "Visual direction",
    output: "Nine-slice interface kit",
    note: "The panel and buttons passed the workflow's judge. The icons are retained as a study because their palette differs from the requested direction.",
  },
] as const;

export type Workflow = (typeof workflows)[number];

export function workflowAsset(workflow: Workflow, file: string): string {
  return workflowAssetPath(workflow.slug, file);
}
