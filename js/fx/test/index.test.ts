import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import * as fx from "../src/index.js";
import { packageRoot } from "./support.js";

test("the package names its version, the one its manifest states", () => {
  const manifest = JSON.parse(readFileSync(join(packageRoot, "package.json"), "utf8"));
  expect(fx.version).toBe(manifest.version);
  expect(fx.version).toMatch(/^\d+\.\d+\.\d+/);
});

test("the SDK's surface", () => {
  for (const name of [
    "binary",
    "plan",
    "price",
    "expand",
    "identity",
    "run",
    "project",
    "inspect",
    "inspectControl",
    "cancel",
    "engineVersion",
    "toYaml",
    "workflow",
    "expr",
  ]) {
    expect(typeof (fx as Record<string, unknown>)[name]).toBe("function");
  }
  expect(new fx.FxError("x")).toBeInstanceOf(Error);
});

test("a workflow document, written for the engine", () => {
  const icon = fx.workflow({
    id: "icon",
    title: "One item icon",
    inputs: { name: { type: "string", description: "What the item is" } },
    steps: {
      draw: {
        uses: "fx/image.generate@1",
        with: { prompt: `A single ${fx.expr("inputs.name")} game icon`, size: "1024x1024" },
      },
    },
    outputs: { icon: fx.expr("steps.draw.outputs.image") },
  });
  expect(fx.toYaml(icon)).toBe(
    [
      "fx: workflow/v1",
      "id: icon",
      "title: One item icon",
      "inputs:",
      "  name:",
      "    type: string",
      "    description: What the item is",
      "steps:",
      "  draw:",
      "    uses: fx/image.generate@1",
      "    with:",
      "      prompt: A single ${{ inputs.name }} game icon",
      "      size: 1024x1024",
      "outputs:",
      "  icon: ${{ steps.draw.outputs.image }}",
      "",
    ].join("\n"),
  );
});
