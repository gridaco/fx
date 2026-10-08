import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { call } from "../src/engine.js";
import { cancel, FxError, inspectControl, RunControlError } from "../src/index.js";
import type { CancelOptions, RunControlResult } from "../src/index.js";
import { SLOW, temporaryFolder } from "./support.js";

const NODE = spawnSync("node", ["--version"]).status === 0;
const pause = (ms: number) => new Promise<void>((done) => setTimeout(done, ms));
const SCRIPT = `#!/usr/bin/env node
const fs = require("node:fs");
const config = JSON.parse(fs.readFileSync(process.env.FAKE_CONTROL_CONFIG, "utf8"));
const log = (value) => fs.appendFileSync(process.env.FAKE_CONTROL_LOG, JSON.stringify(value) + "\\n");
log({args:process.argv.slice(2),source:process.env.GRIDA_FX_CANCEL_SOURCE ?? null});
if (config.sleep) {
  let interrupted = false;
  const stop = (signal) => {
    log({signal});
    interrupted = true;
    if (!config.gate) process.exit(130);
  };
  process.on("SIGINT", () => stop("SIGINT"));
  process.on("SIGTERM", () => stop("SIGTERM"));
  fs.writeFileSync(config.ready, String(process.pid));
  setInterval(() => {
    if (interrupted && fs.existsSync(config.gate)) {
      log({cleanup:"complete"});
      process.exit(130);
    }
  }, 10);
} else {
  process.stdout.write(config.stdout ?? "");
  process.stderr.write(config.stderr ?? "");
  process.exit(config.status ?? 0);
}
`;

function fake() {
  const folder = temporaryFolder("control");
  const engine = join(folder.path, "fake-engine");
  const config = join(folder.path, "config.json");
  const log = join(folder.path, "log.jsonl");
  writeFileSync(engine, SCRIPT);
  chmodSync(engine, 0o755);
  return {
    ...folder,
    options: { cwd: folder.path, env: { GRIDA_FX_BIN: engine, FAKE_CONTROL_CONFIG: config, FAKE_CONTROL_LOG: log } },
    reply: (reply: Record<string, unknown>) => writeFileSync(config, JSON.stringify(reply)),
    log: (): Record<string, unknown>[] => existsSync(log)
      ? readFileSync(log, "utf8").trim().split("\n").map((line) => JSON.parse(line))
      : [],
  };
}

function result(operation: "inspect" | "cancel" = "cancel", fields: Record<string, unknown> = {}): RunControlResult {
  return {
    kind: "fx-run-control-v1",
    operation,
    invocation_id: "inv-1",
    outcome: operation === "inspect" ? "inspected" : "accepted",
    request_status: operation === "inspect" ? "not_accepted" : "accepted",
    recorded_state: "unfinished",
    cleanup: "pending",
    external_completion: "not_verified",
    ...(operation === "inspect" ? { availability: "available", can_cancel: true } : {}),
    ...fields,
  } as RunControlResult;
}

async function until(check: () => boolean): Promise<void> {
  const deadline = Date.now() + 10_000;
  while (!check()) {
    if (Date.now() >= deadline) throw new Error("fake engine did not become ready");
    await pause(10);
  }
}

describe.skipIf(!NODE || process.platform === "win32")("exact-target run control", () => {
  test("forwards options and preserves inspect, acceptance and terminal result", async () => {
    const engine = fake();
    try {
      engine.reply({ stdout: JSON.stringify(result("inspect")) });
      const sampled = await inspectControl("gallery/one", engine.options);
      expect(sampled.invocation_id).toBe("inv-1");
      expect(sampled.can_cancel).toBe(true);
      expect(engine.log().at(-1)?.args).toEqual(["inspect", "gallery/one", "--control", "--json"]);
      engine.reply({ stdout: JSON.stringify(result()) });
      const requested = await cancel("runs/one", { ...engine.options, invocation: sampled.invocation_id ?? "" });
      expect(requested.cleanup).toBe("pending");
      expect(engine.log().at(-1)?.args).toEqual(["cancel", "runs/one", "--source=sdk", "--invocation=inv-1", "--json"]);
      engine.reply({ stdout: JSON.stringify(result("cancel", { outcome: "completed", recorded_state: "failed", cleanup: "complete" })) });
      const completed = await cancel("runs/one", { ...engine.options, wait: true, timeout: "2m" });
      expect(completed.recorded_state).toBe("failed");
      expect(completed.external_completion).toBe("not_verified");
      expect(engine.log().at(-1)?.args).toEqual(["cancel", "runs/one", "--source=sdk", "--wait", "--timeout=2m", "--json"]);
    } finally {
      engine.cleanup();
    }
  });

  test("a stable failure retains acceptance and cleanup evidence", async () => {
    const engine = fake();
    try {
      engine.reply({ stdout: JSON.stringify(result("cancel", { outcome: "error", code: "wait_timeout", message: "local cleanup is still pending" })), status: 1 });
      const error = await cancel("runs/one", { ...engine.options, wait: true }).catch((caught: unknown) => caught);
      expect(error).toBeInstanceOf(RunControlError);
      expect((error as RunControlError).code).toBe("wait_timeout");
      expect((error as RunControlError).exitCode).toBe(1);
      expect((error as RunControlError).result.request_status).toBe("accepted");
      expect((error as RunControlError).result.cleanup).toBe("pending");
    } finally {
      engine.cleanup();
    }
  });

  test("future, malformed and mismatched responses are never success", async () => {
    const engine = fake();
    try {
      for (const [fields, status, wait] of [
        [{ outcome: "future_success" }, 0, false],
        [{ outcome: "accepted" }, 2, false],
        [{ outcome: "completed" }, 0, false],
        [{ outcome: "accepted" }, 0, true],
        [{ outcome: "completed", cleanup: "pending", recorded_state: "cancelled" }, 0, true],
        [{ outcome: "completed", cleanup: "complete", recorded_state: "unfinished" }, 0, true],
        [{ outcome: "completed", cleanup: "complete", recorded_state: "cancelled", invocation_id: null }, 0, true],
        [{ outcome: "error", code: "future_code", message: "unknown" }, 0, false],
        [{ outcome: "error", code: "wait_timeout", message: "timed out" }, 2, true],
        [{ invocation_id: "x".repeat(257) }, 0, false],
        [{ external_completion: "complete" }, 0, false],
      ] as const) {
        engine.reply({ stdout: JSON.stringify(result("cancel", fields)), status });
        const error = await cancel("runs/one", { ...engine.options, wait }).catch((caught: unknown) => caught);
        expect(error).toBeInstanceOf(FxError);
        expect(error).not.toBeInstanceOf(RunControlError);
      }
      engine.reply({ stderr: "grida-fx: unknown command 'cancel'\n", status: 2 });
      await expect(cancel("runs/one", engine.options)).rejects.toThrow("unknown command");
      engine.reply({ stdout: JSON.stringify(result("cancel", { extension: { future: true } })) });
      expect((await cancel("runs/one", engine.options)).outcome).toBe("accepted");
    } finally {
      engine.cleanup();
    }
  });

  test("invalid options fail before engine creation", async () => {
    const engine = fake();
    try {
      for (const options of [{ timeout: "1s" }, { wait: true, timeout: "0s" }, { wait: true, timeout: "1.5s" }, { wait: "yes" }, { invocation: "" }]) {
        await expect(cancel("runs/one", { ...engine.options, ...options } as CancelOptions)).rejects.toBeInstanceOf(TypeError);
      }
      expect(engine.log()).toEqual([]);
    } finally {
      engine.cleanup();
    }
  });

  test("an aborted run stays owned past ten seconds and repeated abort is idempotent", async () => {
    const engine = fake();
    const gate = join(engine.path, "cleanup");
    const ready = join(engine.path, "ready");
    const controller = new AbortController();
    const reason = new Error("stop this run");
    let settled = false;
    engine.reply({ sleep: true, gate, ready });
    const pending = call(["run", "gallery"], { ...engine.options, signal: controller.signal })
      .then(() => { settled = true; }, (error: unknown) => { settled = true; return error; });
    try {
      await until(() => existsSync(ready));
      controller.abort(reason);
      await until(() => engine.log().some((entry) => entry.signal === "SIGTERM"));
      controller.abort(new Error("stop again"));
      controller.signal.dispatchEvent(new Event("abort"));
      await pause(10_100);
      expect(settled).toBe(false);
      expect(engine.log().filter((entry) => "signal" in entry)).toEqual([{ signal: "SIGTERM" }]);
      expect(engine.log()[0]?.source).toBe("sdk");
    } finally {
      writeFileSync(gate, "release cleanup\n");
      expect(await pending).toBe(reason);
      expect(engine.log()).toContainEqual({ cleanup: "complete" });
      engine.cleanup();
    }
  }, SLOW);

  test("aborting only a control wait ends that client", async () => {
    const engine = fake();
    const ready = join(engine.path, "ready");
    const controller = new AbortController();
    const reason = new Error("stop waiting");
    engine.reply({ sleep: true, ready });
    const pending = cancel("runs/one", { ...engine.options, wait: true, signal: controller.signal })
      .catch((error: unknown) => error);
    try {
      await until(() => existsSync(ready));
      controller.abort(reason);
      expect(await pending).toBe(reason);
      expect(engine.log().filter((entry) => "signal" in entry)).toEqual([{ signal: "SIGINT" }]);
    } finally {
      engine.cleanup();
    }
  }, SLOW);
});
