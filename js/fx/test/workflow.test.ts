import { expect, test } from "bun:test";
import { type VideoPreview, toYaml, workflow } from "../src/index.js";

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
