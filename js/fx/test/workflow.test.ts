import { expect, test } from "bun:test";
import { type Note, type VideoPreview, toYaml, workflow } from "../src/index.js";

test("root and group notes preserve literal content and external file declarations", () => {
  const note: Note = { text: "${{ inputs.prompt }}", format: "text" };
  const doc = workflow({ id: "notes", title: "Notes",
    notes: { literal: note, overview: { file: "./notes/setup.md" } },
    steps: { section: { notes: { help: { about: "draw", text: "Use a **clear silhouette**." } },
      steps: { draw: { uses: "fx/image.generate@1", with: { prompt: "A geometric lantern" } } } } },
  });
  expect(doc.notes?.literal).toEqual(note);
  expect(toYaml(doc)).toContain("${{ inputs.prompt }}");
  expect(toYaml(doc)).toContain("file: ./notes/setup.md");
});

test("video preview selectors stay literal in authored workflow documents", () => {
  const preview: VideoPreview = { video: "native", poster: "cover" };
  const document = workflow({
    id: "clip",
    title: "A clip",
    steps: {
      make: { uses: "./nodes/clip.py#clip", preview },
      other: { uses: "./nodes/clip.py#clip", preview: { video: "native" } },
    },
  });
  expect(document.steps.make).toEqual({ uses: "./nodes/clip.py#clip", preview });
  expect(document.steps.other).toEqual({
    uses: "./nodes/clip.py#clip", preview: { video: "native" },
  });
  expect(toYaml(document)).toContain("    preview:\n      video: native\n      poster: cover\n");
});
