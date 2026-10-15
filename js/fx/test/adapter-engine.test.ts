// Project adapters in TypeScript against the real engine (spec/adapters.md §9, spec/protocol.md
// §5.8, §6.8 and §8.2). Planning loads the adapters on the TypeScript planning host, and a live run
// starts an adapter host of this package (compiled into a temporary node_modules/@grida/fx) for
// each declaration. Skipped without a built engine, or where the TypeScript node host cannot run.
// The project is a scratch one: two declared providers, `acme` (with a made-up key) served by a
// request adapter and `acme-jobs` (keyless) by a long-job adapter of the same module, each on a
// capped structured.generate route. The network is off, so every `fetch` is refused before it
// leaves, and no provider key of FX's own is in the environment. The cases follow
// python/tests/test_adapter_engine.py.

import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { FxError, plan, run } from "../src/index.js";
import { HOST_SKIP, makeProject } from "./host-support.js";
import { SLOW, testEngine } from "./support.js";

const engine = testEngine();

/** The keys of FX's built-in providers (spec/providers.md §1), kept out of the runs. */
const KEYS = ["GG_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY", "FAL_KEY", "TRIPO_API_KEY", "ELEVENLABS_API_KEY"];
/** A made-up key for `acme`, which the run's records must never show. */
const MADE_UP_KEY = "acme-made-up-key-3b9e";

const PROJECT = `fx: project/v1
route_tables: [routes.yaml]
providers:
  acme:
    credential: { variable: ACME_API_KEY }
    base_url: { default: "https://api.acme.test/v1" }
    adapter: adapters/acme.ts#acme
  acme-jobs:
    base_url: { default: "https://jobs.acme.test/v1" }
    adapter: adapters/acme.ts#acmeJobs
`;

const ROUTES = `fx: routes/v1
routes:
  - { capability: structured.generate, route: text-a@acme, price: { cap_usd: 0.05 } }
  - { capability: structured.generate, route: text-b@acme-jobs, price: { cap_usd: 0.05 } }
`;

const WORKFLOW = `fx: workflow/v1
id: colours
title: Colours
steps:
  name:
    uses: fx/structured.generate@1
    route: text-a@acme
    with: { prompt: Name a colour., schema: ./schemas/colour.json }
  slowly:
    uses: fx/structured.generate@1
    route: text-b@acme-jobs
    with: { prompt: Name a colour slowly., schema: ./schemas/colour.json }
  refused:
    uses: fx/structured.generate@1
    route: text-a@acme
    with: { prompt: refuse, schema: ./schemas/colour.json }
outputs:
  name: \${{ steps.name.outputs.json }}
  slowly: \${{ steps.slowly.outputs.json }}
`;

const ADAPTER = `import { basename } from "node:path";
import { Answered, adapter, FetchError } from "@grida/fx";

const seen = () => ({ key: "ACME_API_KEY" in process.env, cwd: basename(process.cwd()) });

export const acme = adapter({
  requests: ["structured.generate"],
  send: async (call) => {
    if (call.request.prompt === "refuse") {
      return { outcome: "refused", reason: \`no colours for \${process.env.ACME_API_KEY}\` };
    }
    let fetched: string[] | null = null;
    try {
      await call.fetch(\`\${call.baseUrl}/chat\`, { method: "POST", json: { model: call.route.model }, credential: true });
    } catch (error) {
      if (!(error instanceof FetchError)) {
        throw error;
      }
      fetched = [error.phase, error.reason];
    }
    const answer = { name: "teal", fetched, route: call.route.contract, ...seen() };
    return Answered.json(answer, { costUsd: 0.01, usage: { inputTokens: 5 } });
  },
  version: 1,
});

export const acmeJobs = adapter({
  jobs: ["structured.generate"],
  submit: (call) => ({ outcome: "accepted", handle: { job: call.key.slice(0, 8) } }),
  collect: async (call) =>
    Answered.json({ name: "slate", job: call.handle.job ?? null, route: call.route.contract, ...seen() }, { costUsd: 0.02 }),
  version: 3,
});
`;

function project(): string {
  return makeProject({
    "fx.yaml": PROJECT,
    "routes.yaml": ROUTES,
    "workflows/colours.yaml": WORKFLOW,
    "adapters/acme.ts": ADAPTER,
    "schemas/colour.json": '{"type": "object", "properties": {"name": {"type": "string"}}}',
  });
}

function environment(): Record<string, string | undefined> {
  const env: Record<string, string | undefined> = {
    GRIDA_FX_BIN: engine ?? "",
    GRIDA_FX_NETWORK: "off",
    GRIDA_FX_DISABLE_DOTENV: "1",
    ACME_API_KEY: MADE_UP_KEY,
  };
  for (const key of KEYS) {
    env[key] = undefined;
  }
  return env;
}

/** Every file under `folder`, by path. */
function files(folder: string): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(folder)) {
    const path = join(folder, entry);
    if (statSync(path).isDirectory()) {
      found.push(...files(path));
    } else {
      found.push(path);
    }
  }
  return found;
}

describe.skipIf(engine === null || HOST_SKIP !== null)("TypeScript project adapters, through the engine", () => {
  test(
    "a live run is served by the project's adapters",
    async () => {
      const root = project();
      const env = environment();
      // Planning loads both adapters on the planning host; a plan needs no adapter host.
      const planned = await plan("colours", { cwd: root, env, maxUsd: 1 });
      expect(planned.ok).toBe(true);

      const result = await run("colours", { cwd: root, env, live: true, maxUsd: 1, runDir: "runs/one" });
      expect(result.ok).toBe(false);
      expect(result.failed).toEqual(["refused#1"]);
      // The refusal's reason reaches the record redacted.
      expect(result.steps["refused#1"]?.error).toBe(
        "structured.generate on text-a@acme was refused: no colours for [redacted]",
      );
      // The request adapter answered after a fetch the engine refused (the network is off); its
      // host had its own key. The long job was submitted and collected by the other adapter,
      // whose host had no key of another declaration. Both hosts ran in the project's root, and
      // each route's contract was completed with its adapter's name and version.
      const placed = (path: string) => {
        const step = result.inspection.run.steps.find((entry) => entry.path === path);
        const file = step?.files[0];
        if (file === undefined) {
          throw new Error(`step ${path} placed no file: ${JSON.stringify(step)}`);
        }
        return JSON.parse(readFileSync(join(result.runDir, file.path), "utf8")) as Record<string, unknown>;
      };
      const folder = root.split("/").at(-1);
      expect(placed("name")).toEqual({
        name: "teal",
        fetched: ["refused", "the network is off (GRIDA_FX_NETWORK=off)"],
        route: { adapter: "adapters/acme.ts#acme", adapter_behavior: 1 },
        key: true,
        cwd: folder,
      });
      const slowly = placed("slowly");
      expect(slowly.name).toBe("slate");
      expect(String(slowly.job)).toHaveLength(8);
      expect(slowly.route).toEqual({ adapter: "adapters/acme.ts#acmeJobs", adapter_behavior: 3 });
      expect([slowly.key, slowly.cwd]).toEqual([false, folder]);
      // Each answer is billed at the cost its adapter reported; the refusal at nothing.
      expect(result.chargedUsd).toBeCloseTo(0.03, 9);
      expect(result.steps["name#1"]?.facts?.cost_usd).toBeCloseTo(0.01, 9);
      expect(result.steps["slowly#1"]?.facts?.cost_usd).toBeCloseTo(0.02, 9);
      // The key is in no record of the run, nor in the cache.
      for (const path of [...files(result.runDir), ...files(join(root, ".fx", "cache"))]) {
        expect(readFileSync(path, "latin1").includes(MADE_UP_KEY)).toBe(false);
      }
    },
    SLOW,
  );

  test(
    "an adapter that cannot be loaded refuses the plan",
    async () => {
      const root = project();
      writeFileSync(
        join(root, "adapters", "acme.ts"),
        'import { adapter } from "@grida/fx";\n\nexport const acme = adapter({ requests: ["structured.generate"] } as never);\n',
      );
      let refused: unknown;
      try {
        await plan("colours", { cwd: root, env: environment(), maxUsd: 1 });
      } catch (error) {
        refused = error;
      }
      expect(refused).toBeInstanceOf(FxError);
      expect(String((refused as FxError).message)).toContain(
        "fx.yaml: providers.acme.adapter: adapters/acme.ts failed to import: TypeError: adapter(): requests need send",
      );
    },
    SLOW,
  );
});
