import { expect, test } from "bun:test";
import { parseRunExport } from "@grida/fx-web";
import portrait from "../../../content/showcases/recordings/b35ef1984d95307e9bb16741640d5df33d7786a52fa7933d23502072347cabc1.json";
import { comparisonArtifact, workflowComparisons } from "./comparisons";
import { showcaseLock } from "./catalog";
import type { ShowcaseSlug } from "./contracts";

test("authored comparisons use distinct included images under the pinned recording and delivery base", () => {
  const previous = process.env.FX_SHOWCASE_ASSET_BASE;
  try {
    process.env.FX_SHOWCASE_ASSET_BASE = "https://assets.example.com/media";
    for (const slug of ["parallax", "terrain", "ui-kit", "portrait-motion"] as const) {
      const [comparison] = workflowComparisons(slug);
      expect(comparison.before.src).not.toBe(comparison.after.src);
      const recordingId = { parallax: "meadow-01", terrain: "terrain-01", "ui-kit": "ui-kit-01", "portrait-motion": "nami-face-01" }[slug];
      const recording = showcaseLock.workflows[slug].recordings.find(item => item.id === recordingId)!;
      for (const image of [comparison.before, comparison.after]) {
        expect(image.src).toMatch(new RegExp(`^https://assets\\.example\\.com/media/runs/${recording.sha256}/artifacts/[a-f0-9]{64}\\.png$`));
        expect(image.label.length).toBeGreaterThan(0);
        expect(image.alt.length).toBeGreaterThan(0);
      }
    }
    for (const slug of ["world", "movie-sprite", "character-3d"] satisfies ShowcaseSlug[]) expect(workflowComparisons(slug)).toEqual([]);
    delete process.env.FX_SHOWCASE_ASSET_BASE;
    expect(workflowComparisons("terrain")[0].before.src).toStartWith("/fx/runs/");
  } finally {
    if (previous === undefined) delete process.env.FX_SHOWCASE_ASSET_BASE;
    else process.env.FX_SHOWCASE_ASSET_BASE = previous;
  }
});

test("native comparison selection follows delivery collection keys and refuses missing steps, states or omitted bytes", () => {
  const document = parseRunExport(structuredClone(portrait));
  const source = { node: "deliver.render#1", output: "states", key: "eyes_closed--mouth_smile" };
  const selected = comparisonArtifact(document, source);
  expect(selected.digest).toBe("f204be69e0d29ff36431f80c7897d8c8d455b97c4269d94de0ee7df721553083");
  const working = comparisonArtifact(document, { ...source, node: "compose.composition#1" });
  expect(selected.digest).not.toBe(working.digest);
  expect(comparisonArtifact(document, { ...source, key: "rest--rest" }).digest).toBe("0512a6989c7bae7b1d8c1463d7ff8270070b07312397b00b548a9dcd2f287e77");
  const states = document.view.nodes.find(node => node.id === source.node)!.outputs.states as { collection: unknown[] };
  states.collection.reverse();
  expect(comparisonArtifact(document, source).digest).toBe(selected.digest);
  expect(() => comparisonArtifact(document, { ...source, node: "missing#1" })).toThrow("Missing comparison step");
  expect(() => comparisonArtifact(document, { ...source, key: "missing-state" })).toThrow("comparison collection entry");
  selected.path = null;
  selected.omitted_reason = "unplaced_input";
  expect(() => comparisonArtifact(document, source)).toThrow("Unavailable comparison image");
});

test("the portrait page uses the exact native 2048 delivery states from its named Nami recording", () => {
  const comparison = workflowComparisons("portrait-motion")[0];
  const pin = showcaseLock.workflows["portrait-motion"].recordings.find(item => item.id === "nami-face-01")!;
  expect(pin.sha256).toBe("b35ef1984d95307e9bb16741640d5df33d7786a52fa7933d23502072347cabc1");
  expect([comparison.width, comparison.height]).toEqual([2048, 2048]);
  expect(comparison.before.src).toEndWith(`/runs/${pin.sha256}/artifacts/0512a6989c7bae7b1d8c1463d7ff8270070b07312397b00b548a9dcd2f287e77.png`);
  expect(comparison.after.src).toEndWith(`/runs/${pin.sha256}/artifacts/f204be69e0d29ff36431f80c7897d8c8d455b97c4269d94de0ee7df721553083.png`);
});
