import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, cpSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  FxError, loadRun, plan, run, RunObservationError, RunRecord, toYaml, workflow,
} from "../src/index.js";
import type { RunEventBatch } from "../src/index.js";
import { repoRoot, SLOW, temporaryFolder, testEngine } from "./support.js";

const NODE = spawnSync("node", ["--version"]).status === 0;
const pause = (milliseconds: number) => new Promise<void>((done) => setTimeout(done, milliseconds));
const envelope = (event = "future_event", invocation = "inv-1") => ({
  kind: "fx-run-events-v1", event, invocation_id: invocation,
  plan: "a".repeat(64), offset_ms: 0, future_field: { retained: true },
});
const batch = (cursor: string, events = [envelope()], more = false) => ({
  kind: "fx-run-event-batch-v1", cursor, events, has_more: more, future_field: true,
});
const snapshot = () => ({
  kind: "fx-run-snapshot-v1", cursor: "attached", plan: { kind: "fx-graph-v1" },
  events: [envelope()], future_field: true,
});

const SCRIPT = `#!/usr/bin/env node
const fs = require("node:fs");
const path = process.env.FAKE_RECORD_CONFIG;
const config = JSON.parse(fs.readFileSync(path, "utf8"));
const args = process.argv.slice(2);
fs.appendFileSync(process.env.FAKE_RECORD_LOG, JSON.stringify(args) + "\\n");
let response;
if (args[0] === "inspect") response = config.inspection;
else if (config.reply !== undefined) response = config.reply;
else if (args.includes("--snapshot")) response = config.snapshot;
else {
  response = config.batches.shift() ?? {kind:"fx-run-event-batch-v1",cursor:config.cursor ?? "attached",events:[],has_more:false};
  config.cursor = response.cursor;
  fs.writeFileSync(path, JSON.stringify(config));
}
process.stdout.write(typeof response === "string" ? response : JSON.stringify(response));
process.exit(args[0] === "inspect" ? config.inspect_status ?? 0 : config.status ?? 0);
`;

function fake() {
  const folder = temporaryFolder("record");
  const engine = join(folder.path, "fake-engine");
  const config = join(folder.path, "config.json");
  const log = join(folder.path, "log.jsonl");
  writeFileSync(engine, SCRIPT);
  chmodSync(engine, 0o755);
  const inspection = {
    run: { folder: "runs/pinned", workflow: "case", state: "unfinished" as const, charged_usd: null, steps: [] },
  };
  const reply = (fields: Record<string, unknown> = {}) => writeFileSync(config, JSON.stringify({
    inspection, snapshot: snapshot(), batches: [], ...fields,
  }));
  reply();
  return {
    ...folder,
    options: { cwd: folder.path, env: { GRIDA_FX_BIN: engine, FAKE_RECORD_CONFIG: config, FAKE_RECORD_LOG: log } },
    inspection,
    reply,
    log: (): string[][] => existsSync(log) ? readFileSync(log, "utf8").trim().split("\n").map((line) => JSON.parse(line)) : [],
  };
}

describe.skipIf(!NODE || process.platform === "win32")("saved run observation", () => {
  test("pins one resolved folder, preserves extensions and releases the loading signal", async () => {
    const engine = fake();
    const loading = new AbortController();
    try {
      const record = await loadRun("case/one", { ...engine.options, verify: true, signal: loading.signal });
      expect(record).toBeInstanceOf(RunRecord);
      expect(record.runDir).toBe(join(engine.path, "runs/pinned"));
      expect(record.inspection.run.state).toBe("unfinished");
      expect("exitCode" in record).toBe(false);
      loading.abort();
      const sampled = await record.snapshot();
      expect(sampled.events[0]?.future_field).toEqual({ retained: true });
      expect(sampled.future_field).toBe(true);
      const read = await record.events({ after: sampled.cursor, limit: 2 });
      expect(read.events).toEqual([]);
      expect(engine.log()).toEqual([
        ["inspect", "case/one", "--json", "--verify"],
        ["observe", record.runDir, "--snapshot"],
        ["observe", record.runDir, "--after=attached", "--limit=2"],
      ]);
    } finally { engine.cleanup(); }
  }, SLOW);

  test("loads failed verification only with explicit verify and consistent failure evidence", async () => {
    const engine = fake();
    try {
      for (const verify of ["true", null, 1]) {
        await expect(loadRun("case/one", { ...engine.options, verify: verify as unknown as boolean }))
          .rejects.toBeInstanceOf(TypeError);
      }
      expect(engine.log()).toHaveLength(0);
      const failed = {
        ...engine.inspection,
        verification: { verified: false, problems: ["placed file differs"], future_field: true },
        future_field: { retained: true },
      };
      engine.reply({ inspection: failed, inspect_status: 1 });
      const unverified = await loadRun("case/one", engine.options).catch((caught: unknown) => caught);
      expect(unverified).toBeInstanceOf(FxError);
      expect((unverified as FxError).exitCode).toBe(1);
      for (const verification of [
        undefined, { verified: true, problems: ["contradiction"] },
        { verified: false, problems: [] }, { verified: false, problems: [""] },
        { verified: false, problems: [3] }, { verified: "false", problems: ["failure"] },
      ]) {
        engine.reply({ inspection: { ...engine.inspection, verification }, inspect_status: 1 });
        await expect(loadRun("case/one", { ...engine.options, verify: true }))
          .rejects.toBeInstanceOf(FxError);
      }
      engine.reply({ inspection: failed, inspect_status: 1 });
      const record = await loadRun("case/one", { ...engine.options, verify: true });
      expect(record.runDir).toBe(join(engine.path, "runs/pinned"));
      expect(record.inspection).toEqual(failed);
      for (const inspection of [
        "not json", [], null, { run: null }, { run: { folder: "" } }, { run: { folder: "bad\0folder" } },
      ]) {
        engine.reply({ inspection });
        await expect(loadRun("case/one", engine.options)).rejects.toBeInstanceOf(FxError);
      }
    } finally { engine.cleanup(); }
  }, SLOW);

  test("refuses invalid reader options before spawning and rejects malformed wire evidence", async () => {
    const engine = fake();
    try {
      const record = await loadRun("case/one", engine.options);
      for (const options of [{ after: "" }, { after: "a\0b" }, { limit: 0 }, { limit: 1.5 }, { limit: 1025 }]) {
        await expect(record.events(options)).rejects.toBeInstanceOf(TypeError);
      }
      expect(engine.log()).toHaveLength(1);
      for (const reply of [
        { ...batch("one"), kind: "future_batch" }, { ...batch("one"), has_more: "yes" },
        { ...batch("one"), cursor: "" }, { ...batch("one"), events: [{ ...envelope(), offset_ms: -1 }] },
        { ...batch("one"), cursor: "nul\0cursor" }, batch("one", [], true),
        { ...batch("one"), events: [{ ...envelope(), kind: "future_events" }] },
        { ...batch("one"), events: [{ ...envelope(), plan: "bad" }] },
        { ...batch("one"), events: [{ ...envelope(), invocation_id: "" }] }, "not json",
      ]) {
        engine.reply({ reply });
        const error = await record.events().catch((caught: unknown) => caught);
        expect(error).toBeInstanceOf(FxError);
        expect(error).not.toBeInstanceOf(RunObservationError);
      }
      engine.reply({ reply: { ...snapshot(), plan: { kind: "future_graph" } } });
      await expect(record.snapshot()).rejects.toBeInstanceOf(FxError);
      for (const reply of [batch("same"), batch("different", []), batch("next", [envelope(), envelope()])]) {
        engine.reply({ reply });
        await expect(record.events({ after: "same", limit: 1 })).rejects.toBeInstanceOf(FxError);
      }
      for (const reply of [
        { kind: "fx-run-observation-error-v1", code: "invalid_cursor", message: "Bad cursor" },
        { kind: "fx-run-observation-error-v1", code: "future_error", message: "Future failure", future_field: 3 },
      ] as const) {
        engine.reply({ reply, status: 2 });
        const error = await record.events({ after: "wrong" }).catch((caught: unknown) => caught);
        expect(error).toBeInstanceOf(RunObservationError);
        expect((error as RunObservationError).code).toBe(reply.code);
        expect((error as RunObservationError).document).toEqual(reply);
      }
      engine.reply({ reply: { kind: "fx-run-observation-error-v1", code: "invalid_cursor", message: "Bad" }, status: 0 });
      await expect(record.events()).rejects.toBeInstanceOf(FxError);
    } finally { engine.cleanup(); }
  }, SLOW);

  test("follow drains without overlap, preserves its cursor against a slow consumer, and crosses resumes", async () => {
    const engine = fake();
    try {
      engine.reply({ batches: [
        batch("first", [envelope("run_finished")], true),
        batch("second", [envelope("run_started", "inv-2")]),
      ] });
      const record = await loadRun("case/one", engine.options);
      const following = record.follow({ after: "attached", limit: 1, pollIntervalMs: 100 });
      const first = (await following.next()).value as RunEventBatch;
      first.cursor = "consumer mutation";
      first.has_more = false;
      await pause(150);
      expect(engine.log()).toHaveLength(2); // No background request while consumer is paused.
      const second = (await following.next()).value as RunEventBatch;
      expect(second.events[0]?.invocation_id).toBe("inv-2");
      expect(engine.log()[2]).toContain("--after=first");
      await following.return(undefined);
      await pause(150);
      expect(engine.log()).toHaveLength(3); // Breaking owns no outstanding timer or request.
    } finally { engine.cleanup(); }
  }, SLOW);

  test("follow abort promptly ends a polling sleep and never sends a run cancellation", async () => {
    const engine = fake();
    const controller = new AbortController();
    const reason = new Error("stop observing");
    try {
      engine.reply({ batches: [batch("first")] });
      const record = await loadRun("case/one", engine.options);
      const following = record.follow({ signal: controller.signal, pollIntervalMs: 10_000 });
      await following.next();
      const waiting = following.next().catch((caught: unknown) => caught);
      await pause(30);
      controller.abort(reason);
      expect(await waiting).toBe(reason);
      expect(engine.log()).toHaveLength(2);
      expect(engine.log().some((args) => args[0] === "cancel")).toBe(false);
      for (const interval of [0, Number.NaN, Number.POSITIVE_INFINITY, 2_147_483_648]) {
        await expect(record.follow({ pollIntervalMs: interval }).next()).rejects.toBeInstanceOf(TypeError);
      }
    } finally { engine.cleanup(); }
  }, SLOW);
});

const engine = testEngine();
const offline = {
  GRIDA_FX_BIN: engine ?? "", GRIDA_FX_DISABLE_DOTENV: "1", GRIDA_FX_NETWORK: "off",
  OPENAI_API_KEY: undefined, OPENROUTER_API_KEY: undefined, FAL_KEY: undefined,
  TRIPO_API_KEY: undefined, ELEVENLABS_API_KEY: undefined,
};

describe.skipIf(engine === null)("named SDK journeys against the engine", () => {
  test("creates, refuses collisions, resumes the same plan, and pins saved reads", async () => {
    const folder = temporaryFolder("named-record");
    try {
      writeFileSync(join(folder.path, "fx.yaml"), "fx: project/v1\n");
      writeFileSync(join(folder.path, "workflow.yaml"), toYaml(workflow({
        id: "named-record", title: "Named record", inputs: { label: { type: "string", default: "one" } },
        steps: { choose: { uses: "fx/select@1", with: { first_of: [] } } },
        outputs: { label: "${{ inputs.label }}" },
      })));
      const options = { cwd: folder.path, env: offline, maxUsd: 0 };
      const planned = await plan("workflow.yaml", options);
      const first = await run(planned, { name: "one" });
      expect(first.ok).toBe(true);
      await expect(run(planned, { name: "one" })).rejects.toBeInstanceOf(FxError);
      await expect(run(planned, { resume: "missing" })).rejects.toBeInstanceOf(FxError);
      const resumed = await run(planned, { resume: "one" });
      expect(resumed.runDir).toBe(first.runDir);
      expect(resumed.invocationId).not.toBe(first.invocationId);
      await expect(run("workflow.yaml", { ...options, inputs: { label: "changed" }, resume: "one" }))
        .rejects.toBeInstanceOf(FxError);
      const record = await loadRun("named-record/one", { ...options, verify: true });
      expect(record.runDir).toBe(first.runDir);
      expect(record.inspection.verification?.verified).toBe(true);
      const attached = await record.snapshot();
      expect(attached.events.filter((event) => event.event === "run_started")).toHaveLength(2);
      await run(planned, { name: "later" });
      expect(record.runDir).toBe(first.runDir);
      expect((await record.events({ after: attached.cursor })).events).toEqual([]);
      const bad = await record.events({ after: "wrong" }).catch((caught: unknown) => caught);
      expect(bad).toBeInstanceOf(RunObservationError);
      expect((bad as RunObservationError).code).toBe("invalid_cursor");
    } finally { folder.cleanup(); }
  }, SLOW);

  test("observes an independently owned gated run and stopping observation leaves it running", async () => {
    const python = join(repoRoot, "python/.venv/bin/python");
    if (!existsSync(python)) return;
    const folder = temporaryFolder("ongoing-record");
    cpSync(join(repoRoot, "fixtures/control"), folder.path, { recursive: true });
    const options = { cwd: folder.path, env: { ...offline, GRIDA_FX_PYTHON: python }, maxUsd: 0 };
    let ended = false;
    const pending = run("workflow.yaml", { ...options, name: "ongoing" });
    const observed = pending.then(() => { ended = true; }, () => { ended = true; });
    try {
      const deadline = Date.now() + 15_000;
      while (!existsSync(join(folder.path, ".control-entered"))) {
        if (ended || Date.now() > deadline) throw new Error("gated workflow did not start");
        await pause(20);
      }
      const record = await loadRun("control-harness/ongoing", options);
      const attached = await record.snapshot();
      expect(attached.events.some((event) => event.event === "node_started")).toBe(true);
      expect(attached.events.some((event) => event.event === "run_finished")).toBe(false);
      const following = record.follow({ limit: 1 });
      await following.next();
      await following.return(undefined);
      expect(ended).toBe(false);
      writeFileSync(join(folder.path, ".control-release"), "release\n");
      const result = await pending;
      expect(result.ok).toBe(true);
      const batch = await record.events({ after: attached.cursor });
      expect(batch.events.some((event) => event.event === "run_finished")).toBe(true);
    } finally {
      writeFileSync(join(folder.path, ".control-release"), "release\n");
      await observed;
      folder.cleanup();
    }
  }, SLOW);
});
