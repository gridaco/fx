// A mutable fake metadata source cannot change the facts between SDK planning and execution.
import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, isAbsolute, join } from "node:path";
import { FxError, Plan, plan, run } from "../src/index.js";
import { makeRequest } from "../src/request.js";
import { temporaryFolder } from "./support.js";

const NODE = spawnSync("node", ["--version"]).status === 0;
const SCRIPT = `#!/usr/bin/env node
const fs = require("node:fs");
const config = JSON.parse(fs.readFileSync(process.env.FAKE_PREPARED_CONFIG, "utf8"));
const args = process.argv.slice(2);
const files = {};
for (const arg of args) {
  if (arg.startsWith("--resolved-models=")) {
    const name = arg.slice("--resolved-models=".length);
    files[name] = JSON.parse(fs.readFileSync(name, "utf8"));
  }
}
fs.appendFileSync(process.env.FAKE_PREPARED_LOG, JSON.stringify({ args, files }) + "\\n");
if (args[0] === "plan" && args.includes("--prepared")) {
  process.stdout.write(JSON.stringify(config.prepared));
} else if (args[0] === "run") {
  if (config.problemRun) {
    process.stdout.write("draw.with.prompt: the current request is invalid\\n");
    process.exit(1);
  }
  if (config.failRun) {
    process.stderr.write("grida-fx: execution refused\\n");
    process.exit(2);
  }
  process.stdout.write("run       runs/prepared\\n");
} else if (args[0] === "project") {
  process.stdout.write(JSON.stringify({ run: { run_finished: { ok: true } } }));
} else if (args[0] === "inspect") {
  process.stdout.write("{}");
} else {
  process.stderr.write("unexpected separate planning operation");
  process.exit(97);
}
`;

function fake() {
  const folder = temporaryFolder("prepared-plan");
  const engine = join(folder.path, "fake-engine");
  const config = join(folder.path, "config.json");
  const log = join(folder.path, "log.jsonl");
  writeFileSync(engine, SCRIPT);
  chmodSync(engine, 0o755);
  return {
    ...folder,
    options: {
      cwd: folder.path,
      env: { GRIDA_FX_BIN: engine, FAKE_PREPARED_CONFIG: config, FAKE_PREPARED_LOG: log },
      inputs: { prompt: "a lantern" },
    },
    reply: (reply: Record<string, unknown>) => writeFileSync(config, JSON.stringify(reply)),
    calls: (): { args: string[]; files: Record<string, unknown> }[] => existsSync(log)
      ? readFileSync(log, "utf8").trim().split("\n").map((line) => JSON.parse(line))
      : [],
    temporaryFiles: () => {
      const calls: { files: Record<string, unknown> }[] = existsSync(log)
        ? readFileSync(log, "utf8").trim().split("\n").map((line) => JSON.parse(line)) : [];
      return [
        ...readdirSync(folder.path).filter((name) => name.startsWith(".grida-fx-inputs-")),
        ...calls.flatMap((call) => Object.keys(call.files).map(dirname)).filter(existsSync),
      ];
    },
  };
}

const MODELS = {
  fx: "routes/v1",
  routes: [{ capability: "image.generate", route: "img-new@acme", price: { cap_usd: 1 } }],
};

function prepared(models: unknown = MODELS) {
  return {
    kind: "fx-prepared-plan-v1",
    graph: {
      kind: "fx-graph-v1" as const,
      workflow: { id: "lantern", title: "Lantern" }, instances: [], pending: [],
      problems: [], estimate: { low_usd: 0, high_usd: 1, ceiling_usd: 2 },
    },
    price: { phases: [], estimate: { low_usd: 0, high_usd: 1 }, ceiling_usd: 2 },
    resolved_models: models,
  };
}

describe.skipIf(!NODE || process.platform === "win32")("prepared SDK plans", () => {
  test("graph and price come from one invocation and run(plan) retains its model facts", async () => {
    const engine = fake();
    try {
      const snapshot = prepared();
      engine.reply({ prepared: snapshot });
      const planned = await plan("lantern", engine.options);
      expect(planned.graph).toEqual(snapshot.graph);
      expect(planned.price).toEqual(snapshot.price);
      expect(engine.calls()).toHaveLength(1);
      expect(engine.calls()[0]?.args.slice(-2)).toEqual(["--json", "--prepared"]);
      expect(engine.temporaryFiles()).toEqual([]);
      engine.reply({ prepared: prepared({ fx: "routes/v1", routes: [] }) });
      const result = await run(planned);
      expect(result.ok).toBe(true);
      expect(engine.calls().map((call) => call.args[0])).toEqual(["plan", "run", "project", "inspect"]);
      const execution = engine.calls()[1];
      expect(Object.values(execution?.files ?? {})).toEqual([MODELS]);
      const name = Object.keys(execution?.files ?? {})[0];
      expect(isAbsolute(name!)).toBe(true);
      expect(basename(dirname(name!)).startsWith("grida-fx-models-")).toBe(true);
      expect(existsSync(dirname(name!))).toBe(false);
      expect(execution?.args).toContain(`--resolved-models=${name}`);
      expect(engine.temporaryFiles()).toEqual([]);
    } finally {
      engine.cleanup();
    }
  });

  test("prepared unknown forecasts retain their facts and strict planning policy", async () => {
    const engine = fake();
    try {
      const forecast = { low_usd: null, high_usd: null, known_low_usd: 0.04, known_high_usd: 0.04, unpriced_calls: 1 };
      const models = { fx: "routes/v1", routes: [{ capability: "image.generate", route: "img-new@acme" }] };
      const snapshot = prepared(models);
      const uncertain = {
        ...snapshot,
        graph: { ...snapshot.graph, kind: "fx-graph-v2", estimate: { ...forecast, ceiling_usd: 2 } },
        price: { ...snapshot.price, kind: "fx-price-v2", estimate: forecast, phases: [{ phase: 1, steps: 2, calls: [2, 2], ...forecast, then: [] }] },
      };
      engine.reply({ prepared: uncertain });
      const planned = await plan("lantern", { ...engine.options, requireEstimates: true });
      expect(planned.estimate.lowUsd).toBeNull();
      expect(planned.forecast.unpricedCalls).toBe(1);
      await run(planned);
      expect(engine.calls().filter((call) => ["plan", "run"].includes(call.args[0]!)).every((call) => call.args.includes("--require-estimates"))).toBe(true);
      expect(Object.values(engine.calls()[1]?.files ?? {})).toEqual([models]);
      for (const change of [{ unpriced_calls: 0 }, { unpriced_calls: true }, { known_low_usd: -1 }, { high_usd: 1 }]) {
        engine.reply({ prepared: { ...uncertain, graph: { ...uncertain.graph, estimate: { ...uncertain.graph.estimate, ...change } } } });
        await expect(plan("lantern", engine.options)).rejects.toBeInstanceOf(FxError);
      }
    } finally {
      engine.cleanup();
    }
  });

  test("run(target) uses its prepared snapshot and removes files when execution fails", async () => {
    const engine = fake();
    try {
      engine.reply({ prepared: prepared(), failRun: true });
      await expect(run("lantern", engine.options)).rejects.toThrow("execution refused");
      expect(engine.calls().map((call) => call.args[0])).toEqual(["plan", "run"]);
      expect(Object.values(engine.calls()[1]?.files ?? {})).toEqual([MODELS]);
      expect(engine.temporaryFiles()).toEqual([]);
    } finally {
      engine.cleanup();
    }
  });

  test("an empty resolved snapshot is forwarded too", async () => {
    const engine = fake();
    try {
      const empty = { fx: "routes/v1", routes: [] };
      engine.reply({ prepared: prepared(empty) });
      await run("lantern", engine.options);
      expect(Object.values(engine.calls()[1]?.files ?? {})).toEqual([empty]);
      expect(engine.temporaryFiles()).toEqual([]);
    } finally {
      engine.cleanup();
    }
  });

  test("a constructed Plan cannot enable fresh discovery", async () => {
    const engine = fake();
    try {
      const snapshot = prepared();
      engine.reply({ prepared: snapshot });
      const constructed = new Plan(snapshot.graph, snapshot.price, makeRequest("lantern", engine.options));
      await run(constructed);
      expect(engine.calls().map((call) => call.args[0])).toEqual(["run", "project", "inspect"]);
      expect(Object.values(engine.calls()[0]?.files ?? {})).toEqual([{ fx: "routes/v1", routes: [] }]);
      expect(engine.temporaryFiles()).toEqual([]);
    } finally {
      engine.cleanup();
    }
  });

  test("malformed prepared envelopes fail before execution", async () => {
    const engine = fake();
    try {
      for (const change of [
        { graph: { ...prepared().graph, kind: ["fx-graph-v1"] } },
        { graph: { ...prepared().graph, estimate: { low_usd: null, high_usd: null, known_low_usd: 0, known_high_usd: 0, unpriced_calls: 1, ceiling_usd: 2 } } },
        { price: { ...prepared().price, estimate: { low_usd: null, high_usd: null, known_low_usd: 0, known_high_usd: 0, unpriced_calls: 1 } } },
        { kind: "fx-prepared-plan-v2" }, { graph: [] }, { price: [] },
        { graph: { kind: "fx-graph-v1" } },
        { graph: { ...prepared().graph, problems: {} } },
        { graph: { ...prepared().graph, problems: [{ where: 3, message: "invalid" }] } },
        { graph: { ...prepared().graph, pending: {} } },
        { graph: { ...prepared().graph, estimate: { low_usd: 2, high_usd: 1, ceiling_usd: 2 } } },
        { price: {} },
        { price: { ...prepared().price, estimate: { low_usd: -1, high_usd: 2 } } },
        { price: { ...prepared().price, estimate: { low_usd: 0, high_usd: Infinity } } },
        { price: { ...prepared().price, estimate: { low_usd: false, high_usd: 1 } } },
        { price: { ...prepared().price, phases: [null] } },
        { price: { ...prepared().price, ceiling_usd: "2" } },
        { resolved_models: { fx: "routes/v2", routes: [] } },
        { resolved_models: { fx: "routes/v1", routes: {} } },
      ]) {
        engine.reply({ prepared: { ...prepared(), ...change } });
        await expect(plan("lantern", engine.options)).rejects.toBeInstanceOf(FxError);
        expect(engine.temporaryFiles()).toEqual([]);
      }
      expect(engine.calls().every((call) => call.args[0] === "plan")).toBe(true);
    } finally {
      engine.cleanup();
    }
  });

  for (const named of [false, true]) {
    for (const withPlan of [false, true]) {
      test(`delegates ${named ? "named" : "explicit"} resume facts to the engine${withPlan ? " with a Plan" : ""}`, async () => {
        const engine = fake();
        try {
          engine.reply({ prepared: prepared() });
          const target = withPlan ? await plan("lantern", engine.options) : "lantern";
          const before = engine.calls().length;
          if (!named) mkdirSync(join(engine.path, "runs/existing"), { recursive: true });
          engine.reply({ prepared: null });
          const selected = named ? { resume: "take-A" } : { runDir: "runs/existing" };
          await run(target, withPlan ? selected : { ...engine.options, ...selected });
          const calls = engine.calls().slice(before);
          expect(calls.map((call) => call.args[0])).toEqual(["run", "project", "inspect"]);
          expect(calls[0]?.files).toEqual({});
          expect(engine.temporaryFiles()).toEqual([]);
        } finally {
          engine.cleanup();
        }
      });
    }
    test(`reports ${named ? "named" : "explicit"} resume problems without a fresh diagnostic plan`, async () => {
      const engine = fake();
      try {
        if (!named) mkdirSync(join(engine.path, "runs/existing"), { recursive: true });
        engine.reply({ prepared: null, problemRun: true });
        const selected = named ? { resume: "take-A" } : { runDir: "runs/existing" };
        await expect(run("lantern", { ...engine.options, ...selected })).rejects.toThrow("the current request is invalid");
        expect(engine.calls().map((call) => call.args[0])).toEqual(["run"]);
      } finally {
        engine.cleanup();
      }
    });
  }
});
