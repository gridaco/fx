// The SDK against the engine, offline: planning verbs, a run whose paid call has no --live (it
// fails its step, spending nothing), and a run answered from a seeded call cache
// (conformance/cache-replay). Skipped when no engine is built (see support.ts).

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  binary,
  engineVersion,
  expand,
  expr,
  FxError,
  identity,
  inspect,
  Plan,
  plan,
  price,
  project,
  RunFile,
  run,
  toYaml,
  version,
  workflow,
} from "../src/index.js";
import { repoRoot, SLOW, temporaryFolder, testEngine } from "./support.js";

const engine = testEngine();

describe.skipIf(engine === null)("the SDK, driving the engine", () => {
  const folder = temporaryFolder("api");
  const cwd = folder.path;
  const env = { GRIDA_FX_BIN: engine ?? "" };
  const options = { cwd, env, routes: ["routes.yaml"] };

  beforeAll(() => {
    mkdirSync(join(cwd, "workflows"));
    writeFileSync(
      join(cwd, "fx.yaml"),
      toYaml({ fx: "project/v1", routes: { "image.generate": "img-a@acme" } }),
    );
    writeFileSync(
      join(cwd, "routes.yaml"),
      toYaml({
        fx: "routes/v1",
        routes: [
          {
            capability: "image.generate",
            route: "img-a@acme",
            price: { low_usd: 0.01, high_usd: 0.04 },
          },
        ],
      }),
    );
    const icon = workflow({
      id: "icon",
      title: "One item icon",
      inputs: { name: { type: "string" } },
      steps: {
        draw: {
          uses: "fx/image.generate@1",
          with: { prompt: `A single ${expr("inputs.name")} game icon` },
        },
      },
      outputs: { icon: expr("steps.draw.outputs.image") },
    });
    writeFileSync(join(cwd, "workflows", "icon.yaml"), toYaml(icon));
    const broken = workflow({
      id: "broken",
      title: "Needs a feature its route lacks",
      steps: { draw: { uses: "fx/image.generate@1", requires: ["alpha"], with: { prompt: "x" } } },
    });
    writeFileSync(join(cwd, "workflows", "broken.yaml"), toYaml(broken));
  }, SLOW);
  afterAll(() => folder.cleanup());

  test(
    "binary() and the engine's version",
    async () => {
      expect(binary({ env })).toBe(engine as string);
      expect(await engineVersion({ env })).toBe(version);
    },
    SLOW,
  );

  test(
    "expand, identity and price",
    async () => {
      const graph = await expand("workflows/icon.yaml", {
        ...options,
        inputs: { name: "copper lantern" },
      });
      expect(graph.kind).toBe("fx-graph-v1");
      expect(graph.workflow.id).toBe("icon");
      const [draw] = graph.instances;
      expect(draw?.id).toBe("draw#1");
      expect(draw?.with.prompt).toBe("A single copper lantern game icon");
      expect(draw?.routes["image.generate"]?.route).toBe("img-a@acme");
      const identities = await identity("workflows/icon.yaml", {
        ...options,
        inputs: { name: "copper lantern" },
      });
      expect(identities["draw#1"]).toMatch(/^[0-9a-f]{64}$/);
      expect(identities["draw#1"]).toBe(draw?.identity ?? "");
      const priced = await price("icon", { ...options, inputs: { name: "x" }, maxUsd: 3 });
      expect(priced.estimate).toEqual({ low_usd: 0.01, high_usd: 0.04 });
      expect(priced.ceiling_usd).toBe(3);
      expect(priced.phases[0]?.calls).toEqual([1, 1]);
      // The temporary inputs files are gone.
      expect(readdirSync(cwd).filter((name) => name.startsWith(".grida-fx-inputs-"))).toEqual([]);
    },
    SLOW,
  );

  test(
    "plan",
    async () => {
      const planned = await plan("workflows/icon.yaml", {
        ...options,
        inputs: { name: "lantern" },
      });
      expect(planned).toBeInstanceOf(Plan);
      expect(planned.ok).toBe(true);
      expect(planned.estimate).toEqual({ lowUsd: 0.01, highUsd: 0.04, ceilingUsd: null });
      expect(planned.workflowId).toBe("icon");
      const broken = await plan("workflows/broken.yaml", options);
      expect(broken.ok).toBe(false);
      expect(broken.problems).toEqual([
        { where: "draw.requires", message: "img-a@acme does not support alpha" },
      ]);
      const refused = await run(broken, {}).catch((caught: unknown) => caught);
      expect((refused as Error).message).toBe(
        "the plan is refused:\ndraw.requires: img-a@acme does not support alpha",
      );
      expect(existsSync(join(cwd, "runs", "broken"))).toBe(false);
    },
    SLOW,
  );

  test(
    "the engine's errors reject with FxError, without its prefix",
    async () => {
      const missing = await plan("workflows/icon.yaml", options).catch((caught: unknown) => caught);
      expect((missing as FxError).message).toBe("inputs: 'name' is a required property");
      const error = await plan("workflows/nowhere.yaml", options).catch(
        (caught: unknown) => caught,
      );
      expect(error).toBeInstanceOf(FxError);
      expect((error as FxError).exitCode).toBe(2);
      expect((error as FxError).message).not.toStartWith("grida-fx: ");
      expect((error as FxError).stderr).toStartWith("grida-fx: ");
      const bad = await plan("icon", { ...options, maxUsd: -1 }).catch((caught: unknown) => caught);
      expect((bad as FxError).message).toStartWith("--max-usd -1: ");
    },
    SLOW,
  );

  test(
    "an aborted call rejects",
    async () => {
      const controller = new AbortController();
      controller.abort();
      await expect(plan("icon", { ...options, signal: controller.signal })).rejects.toThrow();
    },
    SLOW,
  );

  test(
    "a run without live spends nothing: the paid step fails",
    async () => {
      const result = await run("workflows/icon.yaml", {
        ...options,
        inputs: { name: "lantern" },
        runDir: "runs/offline",
      });
      expect(result.runDir).toBe(join(cwd, "runs", "offline"));
      expect(result.exitCode).toBe(1);
      expect(result.ok).toBe(false);
      expect(result.incomplete).toBe(false);
      expect(result.failed).toEqual(["draw#1"]);
      expect(result.chargedUsd).toBe(0);
      expect(result.steps["draw#1"]?.state).toBe("failed");
      expect(result.steps["draw#1"]?.error).toContain("is a paid call; run with --live");
      expect(result.inspection.run.state).toBe("failed");
      expect(result.stdout).toContain("run       runs/offline");
    },
    SLOW,
  );

  test(
    "a run in a folder the engine names",
    async () => {
      const result = await run("icon", { ...options, inputs: { name: "lantern" } });
      expect(result.runDir.startsWith(join(cwd, "runs", "icon"))).toBe(true);
      expect(existsSync(join(result.runDir, "events.jsonl"))).toBe(true);
      const latest = await inspect("icon", { cwd, env });
      expect(latest.run.workflow).toBe("icon");
    },
    SLOW,
  );

  test(
    "a plan runs as planned",
    async () => {
      const planned = await plan("icon", { ...options, inputs: { name: "lantern" } });
      const result = await run(planned, { runDir: "runs/planned" });
      expect(result.failed).toEqual(["draw#1"]);
    },
    SLOW,
  );

  test(
    "a run answered from the call cache, with its files",
    async () => {
      const replay = temporaryFolder("replay");
      try {
        cpSync(join(repoRoot, "conformance", "cache-replay", "in"), replay.path, {
          recursive: true,
        });
        const result = await run("case", {
          cwd: replay.path,
          env,
          routes: ["routes.yaml"],
          runDir: "runs/one",
        });
        expect(result.ok).toBe(true);
        expect(result.exitCode).toBe(0);
        expect(result.chargedUsd).toBe(0);
        const image = result.outputs.image;
        expect(image).toBeInstanceOf(RunFile);
        const file = image as RunFile;
        expect(file.digest).toBe(
          "f3945de0c1182a1b279816f51ef2e79938d04957c7bfde94cd4bf2eb4c2170b4",
        );
        expect(file.kind).toBe("image/png");
        expect(file.path?.startsWith(join(replay.path, "runs", "one", "files"))).toBe(true);
        expect((await file.read()).length).toBe(file.size);
        expect(result.steps["draw#1"]).toMatchObject({ state: "succeeded", cache: "miss" });
        const projection = await project("runs/one", { cwd: replay.path, env });
        expect(projection.run.run_finished?.ok).toBe(true);
        const checked = await inspect("runs/one", { cwd: replay.path, env, verify: true });
        expect(checked.verification).toEqual({ verified: true, problems: [] });
      } finally {
        replay.cleanup();
      }
    },
    SLOW,
  );
});
