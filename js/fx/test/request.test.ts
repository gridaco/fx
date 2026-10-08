import { describe, expect, test } from "bun:test";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { Plan, run } from "../src/api.js";
import type { Graph, Price } from "../src/documents.js";
import { amountText, makeRequest, planningArgs, runArgs, withInputsFile } from "../src/request.js";
import { temporaryFolder } from "./support.js";

describe("arguments", () => {
  test("the target, then inputs files, the inputs, routes, builder arguments and the ceiling", () => {
    const request = makeRequest("workflows/icon.yaml", {
      inputFiles: ["a.yaml", "--odd.yaml"],
      inputs: { name: "copper lantern" },
      routes: ["routes.yaml"],
      args: { level: "levels/docks.toml", count: 3, loud: true },
      maxUsd: 2.5,
      cwd: "/work",
    });
    expect(planningArgs(request, ".inputs.yaml")).toEqual([
      "workflows/icon.yaml",
      "--inputs=a.yaml",
      "--inputs=--odd.yaml",
      "--inputs=.inputs.yaml",
      "--routes=routes.yaml",
      "--arg=level=levels/docks.toml",
      "--arg=count=3",
      "--arg=loud=true",
      "--max-usd=2.5",
    ]);
    expect(planningArgs(makeRequest("icon", {}), null)).toEqual(["icon"]);
    expect(makeRequest("icon", { cwd: "/work/./x/.." }).cwd).toBe("/work");
    expect(makeRequest("icon", { inputs: {} }).inputs).toBeNull();
  });

  test("targets and options of the wrong shape are refused before the engine runs", () => {
    expect(() => makeRequest("", {})).toThrow(TypeError);
    expect(() => makeRequest("-x.yaml", {})).toThrow('cannot start with "-"');
    expect(() => makeRequest("icon", { inputs: [] as unknown as Record<string, unknown> })).toThrow(
      "inputs is an object",
    );
    expect(() => makeRequest("icon", { args: { a: {} as unknown as string } })).toThrow("args.a");
    expect(() => makeRequest("icon", { routes: [""] })).toThrow("routes holds paths");
    expect(() => makeRequest("icon", { maxUsd: Number.NaN })).toThrow("maxUsd is an amount");
  });

  test("live is explicit: only true admits paid calls", () => {
    expect(runArgs({})).toEqual([]);
    expect(runArgs({ live: false })).toEqual([]);
    expect(runArgs({ live: true })).toEqual(["--live"]);
    expect(() => runArgs({ live: "yes" as unknown as boolean })).toThrow(
      'live is true or false, not "yes"',
    );
    expect(() => runArgs({ live: 1 as unknown as boolean })).toThrow("live is true or false");
  });

  test("the run's own options", () => {
    expect(
      runArgs({
        live: true,
        yesUpTo: 1,
        runDir: "runs/one",
        deliver: { icon: "out/icon.png", all: "out/{key}.png" },
      }),
    ).toEqual([
      "--live",
      "--yes-up-to=1",
      "--run=runs/one",
      "--deliver=icon=out/icon.png",
      "--deliver=all=out/{key}.png",
    ]);
    expect(() => runArgs({ runDir: "" })).toThrow("runDir");
    expect(() => runArgs({ deliver: { icon: "" } })).toThrow("deliver.icon");
  });

  test("named creation and named resume are explicit, exclusive selections", () => {
    expect(runArgs({ name: "character_1" })).toEqual(["--name=character_1"]);
    expect(runArgs({ resume: "character_1" })).toEqual(["--resume=character_1"]);
    for (const options of [
      { name: "one", resume: "one" }, { runDir: "runs/one", name: "one" },
      { runDir: "runs/one", resume: "one" },
    ]) expect(() => runArgs(options)).toThrow("mutually exclusive");
    for (const option of ["name", "resume", "runDir"] as const) {
      for (const value of ["", "bad\0value", 12]) {
        expect(() => runArgs({ [option]: value })).toThrow(TypeError);
      }
    }
  });

  test("amounts in decimal digits, never an exponent", () => {
    expect(amountText("maxUsd", 10)).toBe("10");
    expect(amountText("maxUsd", 0.5)).toBe("0.5");
    expect(amountText("maxUsd", 1e-7)).toBe("0.0000001");
    expect(amountText("maxUsd", 1.5e-7)).toBe("0.00000015");
    expect(amountText("maxUsd", 1e21)).toBe("1000000000000000000000");
    expect(amountText("maxUsd", 2.5e21)).toBe("2500000000000000000000");
    expect(amountText("maxUsd", -0)).toBe("0");
    expect(amountText("maxUsd", -1)).toBe("-1");
    expect(amountText("maxUsd", "0.25")).toBe("0.25");
    expect(amountText("maxUsd", 3n)).toBe("3");
    expect(() => amountText("maxUsd", Number.POSITIVE_INFINITY)).toThrow(TypeError);
  });
});

test("input values go through a temporary inputs file in cwd, removed after", async () => {
  const folder = temporaryFolder("inputs");
  try {
    const request = makeRequest("icon", {
      cwd: folder.path,
      inputs: { name: "on", size: 2, files: ["a.png"] },
    });
    const seen = await withInputsFile(request, async (name) => {
      expect(name).toMatch(/^\.grida-fx-inputs-[0-9a-f]{16}\.yaml$/);
      return readFileSync(join(folder.path, name as string), "utf8");
    });
    expect(seen).toBe('name: "on"\nsize: 2\nfiles:\n  - a.png\n');
    expect(readdirSync(folder.path)).toEqual([]);
    // Removed after a failure too.
    await expect(
      withInputsFile(request, async () => {
        throw new Error("boom");
      }),
    ).rejects.toThrow("boom");
    expect(readdirSync(folder.path)).toEqual([]);
    expect(
      await withInputsFile(makeRequest("icon", { cwd: folder.path }), async (name) => name),
    ).toBeNull();
  } finally {
    folder.cleanup();
  }
});

describe("plans", () => {
  const graph = (problems: { where: string; message: string }[]): Graph => ({
    kind: "fx-graph-v1",
    workflow: { id: "icon", title: "Icon" },
    instances: [],
    pending: [],
    estimate: { low_usd: 0.04, high_usd: 0.9, ceiling_usd: 10 },
    problems,
  });
  const priced: Price = {
    phases: [{ phase: 1, steps: 6, calls: [1, 3], low_usd: 0.04, high_usd: 0.9, then: [] }],
    estimate: { low_usd: 0.04, high_usd: 0.9 },
    ceiling_usd: 10,
  };

  test("a plan's accessors", () => {
    const planned = new Plan(graph([]), priced, makeRequest("icon", {}));
    expect(planned.ok).toBe(true);
    expect(planned.estimate).toEqual({ lowUsd: 0.04, highUsd: 0.9, ceilingUsd: 10 });
    expect(planned.phases[0]?.calls).toEqual([1, 3]);
    expect(planned.workflowId).toBe("icon");
  });

  test("a plan runs with what it was planned with", async () => {
    const planned = new Plan(graph([]), priced, makeRequest("icon", {}));
    await expect(run(planned, { maxUsd: 5, cwd: "/elsewhere" })).rejects.toThrow(
      "a Plan runs with the target and options it was planned with, so it takes no maxUsd, cwd",
    );
  });

  test("a plan with problems is refused before anything runs", async () => {
    const planned = new Plan(
      graph([{ where: "steps.draw", message: "no route" }]),
      priced,
      makeRequest("icon", {}),
    );
    expect(planned.ok).toBe(false);
    const refused = await run(planned, {}).catch((error: unknown) => error);
    expect(refused).toBeInstanceOf(Error);
    expect((refused as Error).name).toBe("PlanRefused");
    expect((refused as Error).message).toBe("the plan is refused:\nsteps.draw: no route");
    expect(existsSync("runs")).toBe(false);
  });
});
