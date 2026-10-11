import type { Workflow } from "./workflows";

type Details = {
  introduction: string;
  stages: readonly { title: string; description: string }[];
  outputs: readonly { path: string; description: string }[];
  inputs: string;
  agent: readonly (string | { label: string; path: string; workflow?: Workflow["slug"] })[];
  works: readonly string[];
  related: Workflow["slug"];
};

/** Page prose belongs to each showcase; it does not infer features from a run. */
export const workflowDetails: Record<Workflow["slug"], Details> = {
  "parallax": {
    introduction: "A background becomes a place when its layers move at different speeds. This meadow begins with one concept image, then separates the sky, distance, and foreground into reusable scrolling layers. The camera belongs to the application that uses them.",
    stages: [
      { title: "Direct", description: "Use the concept to establish the palette, shapes, and depth of the scene." },
      { title: "Separate", description: "Draw a far layer, a middle distance, and a foreground with room to overlap." },
      { title: "Repeat", description: "Prepare the edges for horizontal repetition; preserve each layer's transparency." },
      { title: "Compose", description: "Record layer order, offsets, and scroll factors in a small manifest." },
      { title: "Preview", description: "Optionally render the same layers into a looping video, without another provider call." },
    ],
    outputs: [
      { path: "layers/far.png", description: "The distant sky and clouds." },
      { path: "layers/middle.png", description: "The meadow's middle distance." },
      { path: "layers/foreground.png", description: "Transparent foreground grass and flowers." },
      { path: "manifest.json", description: "Canvas size, layer order, and parallax factors." },
    ],
    inputs: "# In examples/looping-parallax/inputs/meadow.yaml\n# Or replace with your own concept image.\nconcept: ../.showcase-inputs/meadow-concept.png\nrender_preview: true",
    agent: ["Build an FX workflow that separates ", {"label": "this concept image", "path": "assets/meadow.png", "workflow": "ui-kit"}, " into layers and prepares them for horizontal scrolling. Keep the foreground transparent, give each layer its own scroll factor, and deliver the images with a composition manifest. Plan offline first, then ask for a spending ceiling before generating layers or repainting any edge."],
    works: ["Illustrated side-scrolling scenes", "Independent layers with transparent foregrounds", "Local changes to offsets, order, and scroll speed"],
    related: "terrain",
  },
  "world": {
    introduction: "A poster gives a world its visual language. A synopsis gives it facts. This workflow keeps those roles separate, proposes the people, places, and objects, then illustrates and reviews each entity. Explore the images alongside the records that explain them.",
    stages: [
      { title: "Ground", description: "Keep the synopsis, visual reference, and expansion direction distinct." },
      { title: "Propose", description: "Describe entities, relationships, tensions, and unanswered questions." },
      { title: "Review", description: "Check the proposed world before paying to illustrate it." },
      { title: "Illustrate", description: "Draw each entity and retain its individual image verdict." },
    ],
    outputs: [
      { path: "artifacts/universe.json", description: "The structured world and its connected entities." },
      { path: "artifacts/entities/entity_mira_vale.png", description: "Mira Vale, the meadow's field caretaker." },
      { path: "world-six-entities.jpg", description: "All six illustrated studies together." },
      { path: "selection.json", description: "The four images admitted by the image judge." },
    ],
    inputs: "universe_id: windward_meadow\ntitle: Where the Meadow Listens\nmedium: anime_2d\nsynopsis: synopsis.md\nposter: ../../.showcase-inputs/meadow.png\ndirection: expansion-direction.md\ncensus: { min_entities: 6, max_entities: 6 }\nrights:\n  status: unreviewed\n  basis:\n    - Original brand-neutral storyworld authored for this example.\n    - The supplied meadow poster is original AI-generated artwork from text only; no third-party image reference.",
    agent: ["Build an FX workflow that turns my synopsis and ", {"label": "this poster", "path": "assets/source-poster.png"}, " into a small illustrated world. Use the synopsis for facts and the poster for visual direction. Review the proposed entities before drawing them, then review each illustration independently. Keep rejected studies and their reasons visible. Plan offline before asking for a live budget."],
    works: ["Small storyworlds and concept collections", "Distinct people, places, and objects", "An image and an inspectable record for each entity"],
    related: "character-3d",
  },
  "movie-sprite": {
    introduction: "A character idle needs more than a generated video. The background has to come away cleanly, the loop has to close, and the face has to stay usable for later expressions. Here, generation and local finishing are separate steps, so the same paid take can be finished again without drawing it again.",
    stages: [
      { title: "Prepare", description: "Fit the original character onto a flat green plate and write one motion brief." },
      { title: "Generate", description: "Make one eight-second video take, keeping the camera and pose steady." },
      { title: "Finish", description: "Trim the noisy endpoints, remove green, hold the face locally, and close the loop." },
      { title: "Reuse", description: "Deliver the transparent loop and its exact first frame for later compositing." },
    ],
    outputs: [
      { path: "assets/loop.webm", description: "The transparent body idle for browser playback." },
      { path: "assets/loop.webp", description: "An animated WebP alternative." },
      { path: "assets/canonical.png", description: "The exact first frame of the finished loop." },
      { path: "assets/contact-sheet.png", description: "Sampled frames for reviewing motion and edges." },
    ],
    inputs: "character: .showcase-inputs/nami.png\n# Full motion direction and constraints are in showcase.yaml.\nseconds: 8\nresolution: 720p\naspect_ratio: \"9:16\"\n# Settings for a new take; the recorded face pin is separate.\nfinish: finish.initial.json",
    agent: ["Build an FX workflow that turns ", {"label": "this character", "path": "assets/source.png"}, " into an eight-second body idle. Keep the face still for separate expressions. Plan offline, then ask me for a spending ceiling before generating a take. Show me the take before finishing it. Make keying, loop closure, and any face pin local steps so I can revise them without paying for another video. Deliver the transparent loop and its exact first frame."],
    works: ["Gentle breathing, hair settling, and small idle motion", "A fixed camera and a character that stays in its pose", "Characters without green clothing or props"],
    related: "portrait-motion",
  },
  "portrait-motion": {
    introduction: "A blink should change the eyes, not redraw the character. Starting with a native face crop, this workflow draws independent eye and mouth states, aligns them, and returns reusable patches. The surrounding pixels and original alpha stay intact.",
    stages: [
      { title: "Locate", description: "Find the face and create a workspace without losing its original position." },
      { title: "Draw", description: "Generate eye and mouth states together in a controlled sheet." },
      { title: "Register", description: "Align each feature and check its boundaries against the source." },
      { title: "Compose", description: "Return six native patches and verify all nine state combinations." },
    ],
    outputs: [
      { path: "assets/face-manifest.json", description: "Patch offsets, state choices, and the authored timeline." },
      { path: "assets/patches/eyes_closed-canvas_left_eye.png", description: "A native patch for the left eye's closed state." },
      { path: "assets/states-review.webp", description: "The nine eye-and-mouth combinations side by side." },
      { path: "assets/animation.webp", description: "The facial timeline as a full-size animation." },
    ],
    inputs: "portrait: .showcase-inputs/nami-face.png\nspec: inputs/face-closeup.json\nface_crop: true",
    agent: ["Build an FX workflow that adds independent eye and mouth states to ", {"label": "this face image", "path": "assets/canonical.png"}, ". Find the face automatically, keep the source pixels outside each patch unchanged, and preserve the original alpha. Deliver native patches, offsets, and a small state manifest. Show me every combination and the animation. Plan offline before requesting a provider budget."],
    works: ["Unobstructed open eyes and a closed resting mouth", "Independent blinks and simple mouth expressions", "Replacing only the selected facial features"],
    related: "movie-sprite",
  },
  "character-3d": {
    introduction: "Nami’s rig-ready illustration guides front and back references, a textured mesh and a skeleton. The recorded trial needed measured local finishing and an explicit lower-body weight correction. Inspect those stages, the corrected model and its diagnostic poses separately.",
    stages: [
      { title: "Reference", description: "Draw and review front and back views guided by Nami’s rig-ready illustration." },
      { title: "Mesh", description: "Build a textured whole-body model from the admitted references." },
      { title: "Assemble", description: "Normalize its orientation, scale, and grounding with local tools." },
      { title: "Rig", description: "Bind a skeleton, correct the measured lower-body weights, and inspect diagnostic poses." },
    ],
    outputs: [
      { path: "assets/model-corrected.glb", description: "The textured, skinned character with six diagnostic clips." },
      { path: "assets/nami-sd.png", description: "The reviewed canonical reference." },
      { path: "rig-six-clips.jpg", description: "Rest, shoulder, elbow, knee, wrist, and cheer poses." },
      { path: "manifest.json", description: "Model stages, review evidence, and qualification status." },
    ],
    inputs: "brief: brief.md\nreference: ../../.showcase-inputs/nami-sd.png\nprofile: sd_human_fixed_hands_v1\npartition: whole\nquality_bar: low\nreview: required\nthrough: rig",
    agent: ["Build an FX workflow for an original adult character with short hair, a compact outfit, and fixed mitten hands. Use ", {"label": "this generated reference", "path": "assets/nami-sd.png"}, " as a guide to proportions. Start from my own brief and make reviewed front and back references, generate a textured whole-body mesh, normalize it locally, then rig it. Inspect diagnostic poses and preserve every review. Check local tool availability and plan offline before asking for a provider ceiling. Do not call pipeline acceptance game-runtime qualification."],
    works: ["Compact adult SD characters with short hair", "Fixed mitten hands and close-fitting clothing", "Inspecting skeleton deformation before engine integration"],
    related: "movie-sprite",
  },
  "terrain": {
    introduction: "Generate a fixed set of 47 terrain tiles, validate the atlas layout, and use a neighborhood lookup to select each tile. The painter uses the delivered atlas and lookup directly.",
    stages: [
      { title: "Direct", description: "Translate the meadow's visual direction into one terrain material." },
      { title: "Generate", description: "Draw the fixed atlas of 47 tiles, one for each supported neighborhood mask." },
      { title: "Validate", description: "Check sheet dimensions, cells, and the fixed layout." },
      { title: "Paint", description: "Resolve occupied cells through the lookup and render a preview locally." },
    ],
    outputs: [
      { path: "assets/atlas.png", description: "The terrain atlas: twelve columns, four rows, 120px cells." },
      { path: "assets/lookup.json", description: "Nine-neighbor masks mapped to atlas cells." },
      { path: "assets/recorded-preview.png", description: "The deterministic layout rendered by this run." },
      { path: "assets/validation.json", description: "The atlas's structural checks." },
    ],
    inputs: "material: Golden meadow soil and grass.\ncell_px: 120\nmask_count: 47\npreview_rows:\n  - '00000000'\n  - '00011000'\n  - '11111111'",
    agent: ["Build an FX workflow that generates a 47-tile terrain atlas from the visual direction of ", {"label": "this meadow", "path": "assets/meadow.png", "workflow": "ui-kit"}, ". Lock the cell geometry before generation, validate the delivered sheet, and produce a deterministic map preview. Keep the atlas lookup and the consumer separate from generation so I can paint new layouts locally. Plan offline before requesting a generation budget."],
    works: ["Block-based side-view terrain", "A fixed atlas geometry and mask convention", "Painting and changing layouts without regenerating art"],
    related: "parallax",
  },
  "ui-kit": {
    introduction: "Interface art has to survive interaction. A panel needs corners that stay crisp as it grows; a button needs states that line up. This workflow draws and validates those sheets, then hands a renderer the measured cells and insets. Try the same art at different sizes below.",
    stages: [
      { title: "Specify", description: "Define panel, button, and icon roles before drawing." },
      { title: "Draw", description: "Generate a coordinated kit from one visual direction." },
      { title: "Measure", description: "Validate cells, corners, insets, and button-state geometry." },
      { title: "Use", description: "Render the nine slices and try the kit's real interaction states." },
    ],
    outputs: [
      { path: "assets/panel_frame.png", description: "The resizable panel frame." },
      { path: "assets/button_rect.png", description: "Normal, hover, pressed, and disabled button states." },
      { path: "assets/preview_icons.png", description: "Sixteen illustrated interface icons." },
      { path: "assets/panel_frame_validation.json", description: "Measured panel cells, insets, and fill rules." },
    ],
    inputs: "roles:\n  - panel_frame\n  - button_rect\n  - preview_icons\nstyle: Warm meadow field journal.\nbutton_states: [normal, hover, pressed, disabled]\nicon_count: 16",
    agent: ["Build an FX workflow for a small interface kit: a nine-slice panel, four aligned button states, and sixteen icons. Use ", {"label": "this meadow", "path": "assets/meadow.png"}, " as the visual reference, validate the sheet geometry, and preserve independent review verdicts. Deliver measured insets and cells so the UI can resize without redrawing its corners. Plan offline before asking for a live generation ceiling."],
    works: ["Small coordinated interface kits", "Panels that resize through measured nine slices", "Consistent pointer, keyboard, and disabled button states"],
    related: "terrain",
  },
};

/** Consumption patterns use the selected bundle's actual relative resources. */
export function assetExample(slug: Workflow["slug"], base: string): { language: string; code: string } {
  if (slug === "movie-sprite") return { language: "HTML", code: `<video autoplay loop muted playsinline\n  poster="${base}/assets/canonical.png">\n  <source src="${base}/assets/loop.webm"\n    type="video/webm">\n</video>\n\n<!-- Animated image alternative -->\n<img src="${base}/assets/loop.webp"\n  alt="Character idle">` };
  if (slug === "character-3d") return { language: "HTML", code: `<!-- Load the bundled model-viewer component first. -->\n<script type="module"\n  src="${base}/vendor/model-viewer.min.js"></script>\n\n<model-viewer camera-controls autoplay\n  animation-name="cheer"\n  src="${base}/assets/model-corrected.glb">\n</model-viewer>` };
  if (slug === "world") return { language: "JavaScript", code: `const base = "${base}";\nconst response = await fetch(base + "/manifest.json");\nconst world = await response.json();\n\nfor (const item of world.entities) {\n  const image = new Image();\n  image.src = base + "/" + item.image;\n  image.alt = item.entity.display_name;\n  document.body.append(image);\n}` };
  if (slug === "parallax") return { language: "JavaScript", code: `const base = "${base}";\nconst scene = await fetch(base + "/manifest.json")\n  .then((response) => response.json());\n\n// In your renderer, each layer moves at its own rate.\nfor (const layer of scene.layers) {\n  const image = new Image();\n  image.src = base + "/" + layer.file;\n  const x = -cameraX * layer.parallax;\n  // Repeat the decoded image horizontally at x.\n}` };
  if (slug === "portrait-motion") return { language: "JavaScript", code: `const base = "${base}";\nconst face = await fetch(base + "/assets/face-manifest.json")\n  .then((response) => response.json());\n\nconst [x, y] = face.offset_xy;\nconst closedEyes = face.patches.filter((patch) =>\n  patch.state_id === "eyes_closed");\n\n// At (x, y), replace RGB only where patch alpha is 255.\n// Preserve the source frame's alpha; do not blend twice.\n// Start from the canonical frame for each selection.` };
  if (slug === "terrain") return { language: "JavaScript", code: `const base = "${base}";\nconst sheet = await fetch(base + "/assets/lookup.json")\n  .then((response) => response.json());\n\n// Bits: NW, N, NE, W, center, E, SW, S, SE.\nconst [column, row] = sheet.lookup["111111111"];\nconst cell = 120;\nctx.drawImage(atlas, column * cell, row * cell,\n  cell, cell, x, y, cell, cell);` };
  return { language: "JavaScript", code: `const base = "${base}";\nconst panel = await fetch(\n  base + "/assets/panel_frame_validation.json"\n).then((response) => response.json());\n\n// Use the measured geometry, not guessed border sizes.\nconsole.log(panel.cells, panel.insets);\nconsole.log(panel.band_fill, panel.draw_scale);\n// Draw fixed corners; fill the edges and center\n// according to the recorded rules.` };
}
