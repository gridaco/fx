import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { FxError, listRuns, removeRuns, RunRemovalError } from "../src/index.js";
import type { ListRunsOptions, RemoveRunsOptions } from "../src/index.js";
import { temporaryFolder } from "./support.js";

const NODE = spawnSync("node", ["--version"]).status === 0;
const SCRIPT = `#!/usr/bin/env node
const fs = require("node:fs");
const config = JSON.parse(fs.readFileSync(process.env.FAKE_RUNS_CONFIG, "utf8"));
fs.appendFileSync(process.env.FAKE_RUNS_LOG, JSON.stringify({args:process.argv.slice(2)}) + "\\n");
process.stdout.write(config.stdout ?? "");
process.stderr.write(config.stderr ?? "");
process.exit(config.status ?? 0);
`;

function fake() {
  const folder = temporaryFolder("runs");
  const engine = join(folder.path, "fake-engine");
  const config = join(folder.path, "config.json");
  const log = join(folder.path, "log.jsonl");
  writeFileSync(engine, SCRIPT);
  chmodSync(engine, 0o755);
  return {
    ...folder,
    options: { cwd: folder.path, env: { GRIDA_FX_BIN: engine, FAKE_RUNS_CONFIG: config, FAKE_RUNS_LOG: log } },
    reply: (reply: Record<string, unknown>) => writeFileSync(config, JSON.stringify(reply)),
    log: (): Record<string, unknown>[] => existsSync(log)
      ? readFileSync(log, "utf8").trim().split("\n").map((line) => JSON.parse(line))
      : [],
  };
}

function run(fields: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    folder: "runs/acme/2026-01-02-1",
    workflow: "acme",
    source: "workflows/acme.yaml",
    name: null,
    placement: "allocated",
    state: "succeeded",
    created_at: "2026-01-02T03:04:05Z",
    charged_usd: 0.25,
    stand_in: false,
    ...fields,
  };
}

function list(runs: Record<string, unknown>[], fields: Record<string, unknown> = {}): string {
  return JSON.stringify({ kind: "fx-run-list-v1", runs, skipped: [], ...fields });
}

function removal(applied: boolean, runs: Record<string, unknown>[], fields: Record<string, unknown> = {}): string {
  return JSON.stringify({ kind: "fx-run-removal-v1", applied, runs, skipped: {}, freed_bytes: 120, cache_bytes: 30, ...fields });
}

function removed(outcome: string, fields: Record<string, unknown> = {}): Record<string, unknown> {
  return run({ outcome, freed_bytes: 120, cache_bytes: 30, ...fields });
}

describe.skipIf(!NODE || process.platform === "win32")("listing and removing runs", () => {
  test("a listing passes the workflow and resolves each folder once", async () => {
    const engine = fake();
    try {
      engine.reply({
        stdout: list(
          [
            run({ future_field: { kept: false } }),
            run({
              folder: "/elsewhere/img-a", workflow: null, source: null, placement: "external", state: "incomplete",
              created_at: null, charged_usd: null, stand_in: true, stopped: "ceiling reached",
            }),
          ],
          { skipped: [{ folder: "runs/acme/x", code: "unreadable_run", message: "no plan" }], extension: true },
        ),
      });
      const runs = await listRuns({ ...engine.options, workflow: "acme" });
      expect(engine.log().at(-1)?.args).toEqual(["runs", "list", "--workflow=acme", "--json"]);
      expect(runs).toEqual([
        {
          folder: "runs/acme/2026-01-02-1",
          runDir: resolve(realpathSync(engine.path), "runs/acme/2026-01-02-1"),
          workflow: "acme",
          source: "workflows/acme.yaml",
          name: null,
          placement: "allocated",
          state: "succeeded",
          created_at: "2026-01-02T03:04:05Z",
          charged_usd: 0.25,
          stand_in: false,
        },
        {
          folder: "/elsewhere/img-a",
          runDir: "/elsewhere/img-a",
          workflow: null,
          source: null,
          name: null,
          placement: "external",
          state: "incomplete",
          created_at: null,
          charged_usd: null,
          stand_in: true,
          stopped: "ceiling reached",
        },
      ]);
      engine.reply({ stdout: list([]) });
      expect(await listRuns(engine.options)).toEqual([]);
      engine.reply({ stdout: list([run()], { skipped: [{ folder: "runs/x", code: "future_code" }, "?"] }) });
      expect(await listRuns(engine.options)).toHaveLength(1);
      expect(engine.log().at(-1)?.args).toEqual(["runs", "list", "--json"]);
    } finally {
      engine.cleanup();
    }
  });

  test("a listing beyond sixteen kilobytes is read whole", async () => {
    const engine = fake();
    try {
      const runs = Array.from({ length: 400 }, (_, index) => run({ folder: `runs/acme/2026-01-02-${index + 1}` }));
      const stdout = list(runs);
      expect(Buffer.byteLength(stdout, "utf8")).toBeGreaterThan(16 * 1024);
      engine.reply({ stdout });
      const read = await listRuns(engine.options);
      expect(read).toHaveLength(400);
      expect(read.at(-1)?.runDir).toBe(resolve(realpathSync(engine.path), "runs/acme/2026-01-02-400"));
    } finally {
      engine.cleanup();
    }
  });

  test("malformed, unknown and failed listings are never success", async () => {
    const engine = fake();
    try {
      for (const reply of [
        { stdout: list([run({ state: "archived" })]) },
        { stdout: list([run({ placement: "remote" })]) },
        { stdout: list([run({ charged_usd: "0.25" })]) },
        { stdout: list([run({ stand_in: "no" })]) },
        { stdout: list([run({ folder: "" })]) },
        { stdout: list([run({ workflow: 7 })]) },
        { stdout: list([run({ stopped: null })]) },
        { stdout: list([run({ created_at: undefined })]) },
        { stdout: list([run()], { skipped: {} }) },
        { stdout: list([run()], { kind: "fx-run-list-v2" }) },
        { stdout: list([run()], { skipped: undefined }) },
        { stdout: list([run()]), status: 1 },
        { stdout: "not json" },
        { stdout: "" },
      ]) {
        engine.reply(reply);
        await expect(listRuns(engine.options)).rejects.toBeInstanceOf(FxError);
      }
      engine.reply({ stderr: "grida-fx: no fx.yaml here or above\n", status: 2 });
      const error = await listRuns(engine.options).catch((caught: unknown) => caught);
      expect(error).toBeInstanceOf(FxError);
      expect((error as FxError).message).toBe("no fx.yaml here or above");
      expect((error as FxError).exitCode).toBe(2);
    } finally {
      engine.cleanup();
    }
  });

  test("a removal names its runs after the separator; a preview passes no --yes", async () => {
    const engine = fake();
    try {
      engine.reply({
        stdout: removal(true, [
          removed("removed"),
          removed("forgotten", {
            folder: "../gone", workflow: null, source: null, placement: "external", state: "missing",
            created_at: null, charged_usd: null, freed_bytes: 0, cache_bytes: 0,
          }),
        ]),
      });
      const done = await removeRuns(["runs/acme/2026-01-02-1", "-odd", "../gone"], engine.options);
      expect(engine.log().at(-1)?.args).toEqual([
        "runs", "remove", "--yes", "--json", "--", "runs/acme/2026-01-02-1", "-odd", "../gone",
      ]);
      expect(done.applied).toBe(true);
      expect(done.freed_bytes).toBe(120);
      expect(done.cache_bytes).toBe(30);
      expect(done.skipped).toEqual({});
      expect(done.runs.map((entry) => [entry.outcome, entry.runDir])).toEqual([
        ["removed", resolve(realpathSync(engine.path), "runs/acme/2026-01-02-1")],
        ["forgotten", resolve(realpathSync(engine.path), "../gone")],
      ]);
      expect(done.runs[0]).not.toHaveProperty("code");

      engine.reply({ stdout: removal(false, [removed("would_remove")], { skipped: { holds_runs: 2 } }) });
      const preview = await removeRuns(["acme/hero"], { ...engine.options, preview: true });
      expect(engine.log().at(-1)?.args).toEqual(["runs", "remove", "--json", "--", "acme/hero"]);
      expect(preview.applied).toBe(false);
      expect(preview.runs[0]?.outcome).toBe("would_remove");
      expect(preview.skipped).toEqual({ holds_runs: 2 });

      engine.reply({ stdout: removal(true, [removed("removed")]) });
      await removeRuns(["acme/hero"], { ...engine.options, preview: false });
      expect(engine.log().at(-1)?.args).toEqual(["runs", "remove", "--yes", "--json", "--", "acme/hero"]);
    } finally {
      engine.cleanup();
    }
  });

  test("a refused or partial run is a result with status 1, not a rejection", async () => {
    const engine = fake();
    try {
      engine.reply({
        status: 1,
        stdout: removal(true, [
          removed("refused", { code: "active", message: "an invocation is running it", freed_bytes: 0, cache_bytes: 0 }),
          removed("partial", { code: "io", message: "the tombstone remains" }),
        ]),
      });
      const done = await removeRuns(["runs/a", "runs/b"], engine.options);
      expect(done.runs.map((entry) => [entry.outcome, entry.code, entry.message])).toEqual([
        ["refused", "active", "an invocation is running it"],
        ["partial", "io", "the tombstone remains"],
      ]);
      engine.reply({
        status: 1,
        stdout: removal(false, [removed("refused", { code: "holds_pick", message: "last holder of a pick" })]),
      });
      const preview = await removeRuns(["runs/a"], { ...engine.options, preview: true });
      expect(preview.runs[0]?.code).toBe("holds_pick");
    } finally {
      engine.cleanup();
    }
  });

  test("a RUN that is not a run rejects with the engine's error document", async () => {
    const engine = fake();
    try {
      const document = {
        kind: "fx-run-removal-v1",
        applied: false,
        error: { code: "not_a_run", message: "runs/acme is not a run", run: "runs/acme", future: 1 },
      };
      engine.reply({ status: 2, stdout: JSON.stringify(document) });
      const error = await removeRuns(["runs/acme"], engine.options).catch((caught: unknown) => caught);
      expect(error).toBeInstanceOf(RunRemovalError);
      expect((error as RunRemovalError).code).toBe("not_a_run");
      expect((error as RunRemovalError).run).toBe("runs/acme");
      expect((error as RunRemovalError).message).toBe("runs/acme is not a run");
      expect((error as RunRemovalError).exitCode).toBe(2);
      expect((error as RunRemovalError).result).toEqual({
        kind: "fx-run-removal-v1",
        applied: false,
        error: { code: "not_a_run", message: "runs/acme is not a run", run: "runs/acme" },
      });
      engine.reply({ status: 2, stdout: JSON.stringify({ ...document, error: { code: "ambiguous_target", message: "two", run: null } }) });
      const ambiguous = await removeRuns(["acme"], { ...engine.options, preview: true }).catch((caught: unknown) => caught);
      expect((ambiguous as RunRemovalError).code).toBe("ambiguous_target");
      expect((ambiguous as RunRemovalError).run).toBeNull();
    } finally {
      engine.cleanup();
    }
  });

  test("usage errors and unknown error documents reject with FxError", async () => {
    const engine = fake();
    try {
      engine.reply({ status: 2, stderr: "grida-fx: unrecognized subcommand 'runs'\n" });
      const usage = await removeRuns(["runs/a"], engine.options).catch((caught: unknown) => caught);
      expect(usage).toBeInstanceOf(FxError);
      expect(usage).not.toBeInstanceOf(RunRemovalError);
      expect((usage as FxError).message).toBe("unrecognized subcommand 'runs'");
      for (const error of [
        { code: "future_code", message: "?", run: "runs/a" },
        { code: "not_a_run", message: 3, run: "runs/a" },
        { code: "not_a_run", message: "no", run: 4 },
      ]) {
        engine.reply({ status: 2, stdout: JSON.stringify({ kind: "fx-run-removal-v1", applied: false, error }) });
        const caught = await removeRuns(["runs/a"], engine.options).catch((reason: unknown) => reason);
        expect(caught).toBeInstanceOf(FxError);
        expect(caught).not.toBeInstanceOf(RunRemovalError);
        expect((caught as FxError).message).not.toContain("may already have taken effect");
      }
    } finally {
      engine.cleanup();
    }
  });

  test("malformed, unknown and mismatched previews are never success", async () => {
    const engine = fake();
    try {
      for (const [stdout, status] of [
        [removal(false, [removed("removed")]), 0],
        [removal(true, [removed("removed")]), 0],
        [removal(false, [removed("would_archive")]), 0],
        [removal(false, [removed("would_remove", { code: "future" })]), 0],
        [removal(false, [removed("would_remove", { message: 5 })]), 0],
        [removal(false, [removed("refused", { message: "no code" })]), 1],
        [removal(false, [removed("refused", { code: "active", message: "running" })]), 0],
        [removal(false, [removed("would_remove")]), 1],
        [removal(false, [removed("would_remove", { state: "archived" })]), 0],
        [removal(false, [removed("would_remove", { freed_bytes: -1 })]), 0],
        [removal(false, [removed("would_remove", { cache_bytes: 1.5 })]), 0],
        [removal(false, [removed("would_remove")], { skipped: { stale: 1 } }), 0],
        [removal(false, [removed("would_remove")], { skipped: { young: 0 } }), 0],
        [removal(false, [removed("would_remove")], { freed_bytes: "120" }), 0],
        [removal(false, [removed("would_remove")], { kind: "fx-run-removal-v2" }), 0],
        [removal(false, [removed("would_remove")], { runs: undefined }), 0],
        [removal(false, [removed("would_remove")], { error: { code: "not_a_run", message: "x", run: null } }), 0],
        ["not json", 0],
        [removal(false, [removed("would_remove")]), 3],
      ] as const) {
        engine.reply({ stdout, status });
        const error = await removeRuns(["runs/a"], { ...engine.options, preview: true }).catch((caught: unknown) => caught);
        expect(error).toBeInstanceOf(FxError);
        expect(error).not.toBeInstanceOf(RunRemovalError);
        expect((error as FxError).message).not.toContain("may already have taken effect");
      }
    } finally {
      engine.cleanup();
    }
  });

  test("an applied removal whose output cannot be read says it may have taken effect", async () => {
    const engine = fake();
    try {
      for (const [stdout, status] of [
        [removal(true, [removed("archived")]), 0],
        [removal(true, [removed("would_remove")]), 0],
        [removal(false, [removed("removed")]), 0],
        [removal(true, [removed("partial")]), 1],
        [removal(true, [removed("removed")]), 1],
        [removal(true, [removed("removed")], { cache_bytes: null }), 0],
        ["{\"kind\":\"fx-run-removal-v1\",\"applied\":tr", 0],
        ["", 130],
      ] as const) {
        engine.reply({ stdout, status });
        const error = await removeRuns(["runs/a"], engine.options).catch((caught: unknown) => caught);
        expect(error).toBeInstanceOf(FxError);
        expect(error).not.toBeInstanceOf(RunRemovalError);
        expect((error as FxError).message).toContain("the removal may already have taken effect");
        expect((error as FxError).stdout).toBe(stdout);
        expect((error as FxError).exitCode).toBe(status);
      }
    } finally {
      engine.cleanup();
    }
  });

  test("invalid input fails before the engine starts", async () => {
    const engine = fake();
    try {
      for (const runs of [[], [""], ["runs/a", "bad\0run"], [3], "runs/a", undefined]) {
        await expect(removeRuns(runs as unknown as string[], engine.options)).rejects.toBeInstanceOf(TypeError);
      }
      await expect(removeRuns(["runs/a"], { ...engine.options, preview: "yes" } as unknown as RemoveRunsOptions))
        .rejects.toBeInstanceOf(TypeError);
      for (const workflow of ["", "ac\0me", 7]) {
        await expect(listRuns({ ...engine.options, workflow } as unknown as ListRunsOptions)).rejects.toBeInstanceOf(TypeError);
      }
      expect(engine.log()).toEqual([]);
    } finally {
      engine.cleanup();
    }
  });
});
