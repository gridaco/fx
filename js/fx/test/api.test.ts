// The SDK against the engine, offline: planning verbs, a run whose paid call has no --live (it
// fails its step, spending nothing), a run answered from a seeded call cache
// (conformance/cache-replay), and a run whose paid call a stand-in in this process answers
// (`run(…, { standIn })`, on a copy of conformance/stand-in-image). Skipped when no engine is built
// (see support.ts).

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { deflateSync } from "node:zlib";
import {
  binary,
  DECLINE,
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
  type StandIn,
  type StandInCall,
  toYaml,
  version,
  workflow,
} from "../src/index.js";
import { repoRoot, SLOW, temporaryFolder, testEngine } from "./support.js";

const engine = testEngine();

/** A PNG of one opaque colour, `width` by `height`. */
function png(width: number, height: number): Uint8Array {
  const chunk = (tag: string, data: Buffer) => {
    const body = Buffer.concat([Buffer.from(tag, "latin1"), data]);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const check = Buffer.alloc(4);
    check.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, check]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8);
  const row = Buffer.concat([Buffer.from([0]), ...Array.from({ length: width }, () => Buffer.from([40, 90, 160]))]);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(Buffer.concat(Array.from({ length: height }, () => row)))),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function crc32(data: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of data) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

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
          { capability: "image.generate", route: "img-x@acme", price: { cap_usd: 1.5 } },
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
    const capped = workflow({
      id: "capped",
      title: "A route with a cap and no price",
      steps: { draw: { uses: "fx/image.generate@1", route: "img-x@acme", with: { prompt: "x" } } },
    });
    writeFileSync(join(cwd, "workflows", "capped.yaml"), toYaml(capped));
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
    "a capped route's call is priced at nothing to its cap, and marked",
    async () => {
      const graph = await expand("workflows/capped.yaml", options);
      expect(graph.instances[0]?.price).toEqual({ low_usd: 0, high_usd: 1.5, capped: true });
      const uncapped = await expand("workflows/icon.yaml", { ...options, inputs: { name: "x" } });
      expect(uncapped.instances[0]?.price).toEqual({ low_usd: 0.01, high_usd: 0.04 });
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

  test.skipIf(process.platform === "win32")(
    "a stand-in in this process answers the paid call; a second run is answered from the stand-in store",
    async () => {
      const standing = temporaryFolder("stand-in");
      try {
        cpSync(join(repoRoot, "conformance", "stand-in-image", "in"), standing.path, { recursive: true });
        const calls: StandInCall[] = [];
        const standIn: StandIn = (call) => {
          calls.push(call);
          if (call.capability !== "image.generate") {
            return DECLINE;
          }
          const [width = 0, height = 0] = String(call.request.size).split("x").map(Number);
          return { files: { image: png(width, height) } };
        };
        const options = { cwd: standing.path, env, routes: ["routes.yaml"], standIn };
        const first = await run("case", { ...options, runDir: "runs/one", deliver: { image: "delivered.png" } });
        expect(first.ok).toBe(true);
        expect(first.exitCode).toBe(0);
        expect(first.standIn).toBe(true);
        expect(first.chargedUsd).toBe(0);
        expect(calls.map((call) => [call.capability, call.route.id, call.request.size, [...call.takes]])).toEqual([
          ["image.generate", "img-a@acme", "64x64", [1]],
        ]);
        const image = first.outputs.image as RunFile;
        expect(image).toBeInstanceOf(RunFile);
        expect(image.kind).toBe("image/png");
        expect(Buffer.from(readFileSync(join(standing.path, "delivered.png")))).toEqual(Buffer.from(png(64, 64)));
        expect(first.steps["draw#1"]).toMatchObject({ state: "succeeded" });
        // The same plan in a new folder: the stand-in store answers, and the stand-in is not asked.
        const second = await run("case", { ...options, runDir: "runs/two" });
        expect(second.ok).toBe(true);
        expect(second.standIn).toBe(true);
        expect(calls).toHaveLength(1);
        expect((second.outputs.image as RunFile).digest).toBe(image.digest);
      } finally {
        standing.cleanup();
      }
    },
    SLOW,
  );
});
