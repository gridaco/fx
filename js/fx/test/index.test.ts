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
    "loadRun",
    "cancel",
    "listRuns",
    "removeRuns",
    "engineVersion",
    "toYaml",
    "workflow",
    "expr",
    "node",
    "tool",
    "adapter",
  ]) {
    expect(typeof (fx as Record<string, unknown>)[name]).toBe("function");
  }
  expect(new fx.FxError("x")).toBeInstanceOf(Error);
  // What node bodies use: their files and errors.
  for (const name of [
    "InputFile",
    "NodeFailure",
    "EngineError",
    "Cancelled",
    "CapabilityError",
    "CapabilityUndeclared",
    "OverBound",
    "NoRoute",
    "NotLive",
    "CeilingExceeded",
    "CallRefused",
    "CallFailed",
    "JobUnsettled",
    "AgentUnfinished",
    "UndeclaredResource",
    "ExpressionError",
    "OutsideWorkDir",
    "UnknownFile",
  ]) {
    expect(typeof (fx as Record<string, unknown>)[name]).toBe("function");
  }
  expect(new fx.CallFailed("x")).toBeInstanceOf(fx.EngineError);
  expect(new fx.NodeFailure("x")).toBeInstanceOf(Error);
  // What agent tools and stand-ins use.
  expect(typeof fx.ToolReply).toBe("function");
  expect(fx.DECLINE as symbol).toBe(Symbol.for("grida.fx.decline"));
  for (const name of ["json", "turn", "submit"] as const) {
    expect(typeof fx.Answer[name]).toBe("function");
  }
  // What project adapters use.
  for (const name of ["AdapterCall", "AdapterFile", "FetchResponse", "FetchError"]) {
    expect(typeof (fx as Record<string, unknown>)[name]).toBe("function");
  }
  expect(new fx.FetchError("refused", "x")).toBeInstanceOf(Error);
  expect(typeof fx.Answered.json).toBe("function");
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
