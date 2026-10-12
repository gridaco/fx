import type { Workflow } from "../workflows";

type Source = {
  folder: string;
  target: string;
  inputs?: string;
  /** Absent when the project's routes are free, as on the user's own Codex. */
  max_usd?: number;
  setup: string;
  note: string;
};

/** Maintained authoring projects; frozen recordings remain separate evidence. */
export const workflowSources = {
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
  return {
    ...source,
    url: `https://github.com/gridaco/fx/tree/main/examples/${source.folder}`,
    command: source.max_usd === undefined
      ? `# From examples/${source.folder} in the FX repository.\n# Install the CLI first: npm install -g grida\n${source.setup}\n\ngrida fx plan ${target}\n\n# Start the local service for the browser viewer.\ngrida fx start --background\n\n# After reviewing the plan, draw it live.\ngrida fx run ${target} --live --open`
      : `# From examples/${source.folder} in the FX repository.\n# Install the CLI first: npm install -g grida\n${source.setup}\n\ngrida fx plan ${target} --max-usd ${source.max_usd}\n\n# Start the local service for the browser viewer.\ngrida fx start --background\n\n# After reviewing the plan, choose a provider spending ceiling.\ngrida fx run ${target} --live --max-usd ${source.max_usd} --open`,
  };
}
