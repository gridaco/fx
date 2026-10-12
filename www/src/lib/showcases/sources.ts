import type { Workflow } from "../workflows";

type Source = {
  folder: string;
  ref?: string;
  target: string;
  inputs?: string;
  /** Absent when the project's routes are free, as on the user's own Codex. */
  max_usd?: number;
  prerequisite?: string;
  setup: string;
  note: string;
};

const videoSourceRef = "7e9657df53c93ceba47f82e729d6c3573efeedfb";

/** Maintained authoring projects; frozen recordings remain separate evidence. */
export const workflowSources = {
  "product-promo": {
    folder: "product-promo",
    ref: videoSourceRef,
    target: "workflow.yaml",
    inputs: "replay-segments.yaml",
    max_usd: 0,
    setup: "uv sync\nsource .venv/bin/activate\npython fetch_inputs.py",
    note: "Install FFmpeg and ffprobe on PATH. The input download retrieves the recorded source media. This recipe replays the selected saved plan locally at $0: caller-authored chronological intervals omit waits and keep the requests and results readable. The original model proposal remains editable. To propose a fresh plan, use workflow.yaml with showcase.yaml and choose a live ceiling; rendering always preserves supplied product pixels.",
  },
  "character-performance": {
    folder: "character-performance",
    ref: videoSourceRef,
    target: "cast.yaml",
    inputs: "inputs/shared-cast.yaml",
    max_usd: 10,
    prerequisite: "Install Grida and build FX from source; select the source build after uv sync below.",
    setup: "uv sync\nsource .venv/bin/activate\n# Before continuing, select the built engine and checkout SDK in this environment:\n# https://github.com/gridaco/fx/blob/4323dd28a67b3d6906444bb1691d78dafa8fcca4/CONTRIBUTING.md#use-a-source-build-with-grida-fx\n# Do not sync dependencies again after selecting the checkout SDK.\npython fetch_inputs.py\npython fetch_motion.py\ngrida fx run prepare.yaml --inputs inputs/riko.yaml --max-usd 0 --deliver character=.showcase-inputs/riko-plate.png --deliver motion=.showcase-inputs/motion-prepared.mp4\ngrida fx run prepare.yaml --inputs inputs/nami.yaml --max-usd 0 --deliver character=.showcase-inputs/nami-plate.png",
    note: "Install FFmpeg and ffprobe on PATH. Use the FX source build containing video.motion_transfer and the Wan Motion route until these are released. Sync this example first, then follow CONTRIBUTING's source-build selection recipe before running the helpers or FX commands; another dependency sync would replace that SDK selection. FX delivers the identity artwork. Acquire the selected stock footage from its source and prepare the four-second, 24 fps interval locally; raw stock footage is excluded from public FX delivery. The cast template runs the single-character utility twice under one spending ceiling. Generation and independent sampled-frame review are paid; decoding, the conservative check of every native frame's studio borders, and composition are local and free. The primary four-second, 1080 × 1920 reel cuts from Riko to Nami at two seconds on their shared driving timeline, at 30 fps. It fits the complete caller-selected common region; the default region is the whole source frame. inputs/cast.yaml retains the eight-second sequential alternate.",
  },
  parallax: {
    folder: "looping-parallax",
    target: "workflows/meadow.yaml",
    inputs: "inputs/meadow.yaml",
    max_usd: 10,
    setup: "pip install grida\npython fetch_showcase_input.py",
    note: "The optional local video preview needs FFmpeg. The download retrieves the recorded concept image; a fresh live run generates new layers.",
  },
  world: {
    folder: "world",
    target: "workflow.yaml",
    inputs: "inputs/windward_meadow/inputs.yaml",
    max_usd: 25,
    setup: "uv sync\nsource .venv/bin/activate\npython ../fetch_showcase_inputs.py .",
    note: "The source includes the synopsis, expansion direction, schemas and custom nodes. The download retrieves the recorded poster.",
  },
  "movie-sprite": {
    folder: "movie-sprite",
    target: "workflow.yaml",
    inputs: "showcase.yaml",
    max_usd: 3,
    setup: "uv sync\nsource .venv/bin/activate\npython ../fetch_showcase_inputs.py .",
    note: "Install FFmpeg and ffprobe on PATH. showcase.yaml generates a wide-frame motion take. finish-showcase.yaml replays the recorded four-second local finish at $0 without a face hold. The input download includes the exact recorded take; earlier quiet presets remain available.",
  },
  pet: {
    folder: "pet",
    target: "workflows/pet.yaml",
    inputs: "inputs/coral.yaml",
    setup: "uv sync\nsource .venv/bin/activate\npython ../fetch_showcase_inputs.py .",
    note: "The download retrieves the recorded reference picture and sheet. This project draws on Codex, signed in with ChatGPT, at $0 to FX; the strips need an image route that takes a reference picture. workflows/reel.yaml renders the cover from a finished sheet with FFmpeg, also at $0.",
  },
  "portrait-motion": {
    folder: "portrait-motion",
    target: "workflow.yaml",
    inputs: "showcase.yaml",
    max_usd: 40,
    setup: "uv sync\nsource .venv/bin/activate\npython ../fetch_showcase_inputs.py .",
    note: "The download retrieves the recorded high-resolution face input. The optional source-workflow.yaml prepares that illustration separately; the facial workflow includes its own state specification and local registration helpers.",
  },
  "character-3d": {
    folder: "character-3d",
    target: "workflow.yaml",
    inputs: "inputs/nami/inputs.yaml",
    max_usd: 30,
    setup: "uv sync\nsource .venv/bin/activate\npython ../fetch_showcase_inputs.py .",
    note: "Install Blender as described in the example. Nami’s rig-ready illustration guides the generated references. The example also includes a provider-free replay of the measured finishing, disclosed weight correction, diagnostics and rendered cover.",
  },
  terrain: {
    folder: "terrain",
    target: "workflow.py:build",
    max_usd: 15,
    setup: "uv sync\nsource .venv/bin/activate\npython ../fetch_showcase_inputs.py .",
    note: "The Python builder reads the included material direction and fixed topology template. The download retrieves the original meadow reference.",
  },
  "ui-kit": {
    folder: "ui-kit",
    target: "workflow.py:build",
    max_usd: 15,
    setup: "uv sync\nsource .venv/bin/activate\npython ../fetch_showcase_inputs.py .",
    note: "The Python builder reads the included ui.json and style direction. The download retrieves the original meadow reference.",
  },
} satisfies Record<Workflow["slug"], Source>;

export function workflowSource(slug: Workflow["slug"]) {
  const source: Source = workflowSources[slug];
  const target = `${source.target}${source.inputs ? ` --inputs ${source.inputs}` : ""}`;
  const prerequisite = source.prerequisite ?? "Install the CLI first: npm install -g grida";
  return {
    ...source,
    url: `https://github.com/gridaco/fx/tree/${source.ref ?? "main"}/examples/${source.folder}`,
    command: source.max_usd === 0
      ? `# From examples/${source.folder} in the FX repository.\n# ${prerequisite}\n${source.setup}\n\ngrida fx plan ${target} --max-usd 0\n\n# Start the local service for the browser viewer.\ngrida fx start --background\n\n# Replay the selected plan locally, without a provider call.\ngrida fx run ${target} --max-usd 0 --open`
      : source.max_usd === undefined
      ? `# From examples/${source.folder} in the FX repository.\n# ${prerequisite}\n${source.setup}\n\ngrida fx plan ${target}\n\n# Start the local service for the browser viewer.\ngrida fx start --background\n\n# After reviewing the plan, draw it live.\ngrida fx run ${target} --live --open`
      : `# From examples/${source.folder} in the FX repository.\n# ${prerequisite}\n${source.setup}\n\ngrida fx plan ${target} --max-usd ${source.max_usd}\n\n# Start the local service for the browser viewer.\ngrida fx start --background\n\n# After reviewing the plan, choose a provider spending ceiling.\ngrida fx run ${target} --live --max-usd ${source.max_usd} --open`,
  };
}
