// The managed native path through real TypeScript hosts; no provider traffic.
import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { setTimeout } from "node:timers/promises";
import { cancel, FxError, inspect, loadRun, plan, run } from "../src/index.js";
import { HOST_SKIP, makeProject } from "./host-support.js";
import { repoRoot, SLOW, testEngine } from "./support.js";

const engine = testEngine();
const KEY = "made-up-native-key-28f3";
const env: Record<string, string | undefined> = {
  GRIDA_FX_BIN: engine ?? "", GRIDA_FX_NETWORK: "off", GRIDA_FX_DISABLE_DOTENV: "1", ACME_NATIVE_KEY: KEY,
  GG_API_KEY: undefined, OPENAI_API_KEY: undefined, OPENROUTER_API_KEY: undefined,
  FAL_KEY: undefined, TRIPO_API_KEY: undefined, ELEVENLABS_API_KEY: undefined,
};
function project(): string {
  const fixture = join(repoRoot, "fixtures/native/typescript");
  return makeProject(Object.fromEntries(readdirSync(fixture).map((name) => [name, readFileSync(join(fixture, name), "utf8")])));
}
function submissions(root: string): { key: string; future: number; source: string; vendor_extension: unknown }[] {
  return readFileSync(join(root, ".native-submits"), "utf8").trim().split("\n").map((line) => JSON.parse(line));
}
function files(folder: string): string[] {
  if (!existsSync(folder)) return [];
  return readdirSync(folder).flatMap((name) => {
    const path = join(folder, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}
const invoke = (cwd: string, folder: string, inputs: Record<string, string | number> = {}) =>
  run("native.yaml", { cwd, env, live: true, maxUsd: 1, runDir: `runs/${folder}`, inputs: { source: "source.txt", ...inputs } });

describe.skipIf(engine === null || HOST_SKIP !== null)("managed native operations", () => {
  test("arbitrary fields and files survive cancellation, collection and independent call-cache replay", async () => {
    const root = project();
    expect((await plan("native.yaml", { cwd: root, env, inputs: { source: "source.txt" } })).ok).toBe(true);
    const pending = invoke(root, "one").catch((error: unknown) => error as FxError);
    const deadline = Date.now() + 20_000;
    try {
      while (!existsSync(join(root, ".native-collecting"))) {
        if (Date.now() > deadline) throw new Error("native collect did not start");
        await Promise.race([setTimeout(20), pending.then((result) => { throw new Error(String(result)); })]);
      }
      expect((await cancel(join(root, "runs/one"), { cwd: root, env, wait: true, timeout: "30s" })).outcome).toBe("completed");
      const stopped = await pending;
      expect(stopped).toBeInstanceOf(FxError);
      expect((stopped as FxError).exitCode).toBe(130);
      const before = await inspect(join(root, "runs/one"), { env });
      expect(submissions(root)).toHaveLength(1);
      expect(submissions(root)[0]).toMatchObject({ future: 70000, source: "Synthetic source bytes.\n", vendor_extension: { new_feature: true } });
      const jobs = files(join(root, ".fx/cache/jobs"));
      expect(jobs).toHaveLength(1);
      expect(JSON.parse(readFileSync(jobs[0]!, "utf8")).state).toBe("submitted");
      writeFileSync(join(root, ".native-release"), "");
      const resumed = await invoke(root, "one");
      expect(resumed.ok).toBe(true);
      expect(resumed.chargedUsd).toBe(before.run.charged_usd ?? 0);
      expect(submissions(root)).toHaveLength(1);
      expect(files(join(root, ".fx/cache/jobs"))).toHaveLength(0);
      const offlineEnv = { ...env, ACME_NATIVE_KEY: undefined };
      expect((await run("native.yaml", { cwd: root, env: offlineEnv, runDir: "runs/cached", inputs: { source: "source.txt" } })).ok).toBe(true);
      const nodes = join(root, "nodes.ts");
      writeFileSync(nodes, readFileSync(nodes, "utf8").replace("version: 1", "version: 2"));
      const replayed = await run("native.yaml", { cwd: root, env: offlineEnv, runDir: "runs/replay", inputs: { source: "source.txt" } });
      expect(replayed.ok).toBe(true);
      const replay = await (await loadRun(replayed.runDir, { env })).snapshot();
      expect(replay.events.some((event) => event.event === "call" && event.cached)).toBe(true);
      expect(submissions(root)).toHaveLength(1);
      expect((await invoke(root, "new-field", { future: 90001 })).ok).toBe(true);
      writeFileSync(join(root, "source.txt"), "Different synthetic source.\n");
      expect((await invoke(root, "new-file", { future: 90001 })).ok).toBe(true);
      expect((await invoke(root, "new-operation", { future: 90001, operation: "future_mesh_operation" })).ok).toBe(true);
      const records = submissions(root);
      expect(records).toHaveLength(4);
      expect(new Set(records.map((record) => record.key)).size).toBe(4);
    } finally {
      writeFileSync(join(root, ".native-release"), "");
      await pending;
    }
  }, SLOW);

  test("provider diagnostics survive SDK, observation, inspection and uncertain-job reconciliation", async () => {
    const root = project();
    const rejected = await invoke(root, "reject", { mode: "reject" });
    expect(rejected.ok).toBe(false);
    const diagnostic = rejected.steps["native#1"]?.diagnostic;
    expect(diagnostic).toMatchObject({ source: "provider", phase: "submit", http_status: 400,
      code: "bad_option", field: "future_face_budget", request_id: "req-native-1", redacted: true });
    expect(rejected.stdout).toContain("Unsupported future_face_budget");
    expect(submissions(root)).toHaveLength(1);
    const record = await loadRun(rejected.runDir, { env });
    const snapshot = await record.snapshot();
    expect(snapshot.events.find((event) => event.event === "node_failed")?.diagnostic).toEqual(diagnostic);
    const inspection = await inspect(rejected.runDir, { env });
    expect((inspection.run.steps[0] as unknown as Record<string, unknown>).diagnostic).toEqual(diagnostic);
    const portable = JSON.stringify(snapshot);
    for (const forbidden of [KEY, "made-up-signature", "/private/native/input.txt", "Synthetic source bytes."]) {
      expect(portable.includes(forbidden)).toBe(false);
    }
    const uncertain = await invoke(root, "uncertain", { mode: "uncertain" });
    const original = uncertain.steps["native#1"]?.diagnostic;
    expect(original?.http_status).toBe(502);
    const again = await invoke(root, "uncertain", { mode: "uncertain" });
    expect(again.steps["native#1"]?.diagnostic).toEqual(original);
    expect(submissions(root)).toHaveLength(2);
    const job = files(join(root, ".fx/cache/jobs")).map((path) => JSON.parse(readFileSync(path, "utf8"))).find((job) => job.state === "submitting");
    expect(job.state).toBe("submitting");
    expect(job.diagnostic).toEqual(original);
  }, SLOW);
});

describe.skipIf(engine === null || HOST_SKIP !== null)("native diagnostic privacy", () => {
  test("identifier-shaped file echoes are withheld from every metadata field and export", async () => {
    for (const field of ["code", "field", "requestId", "taskId", "operation"]) {
      const root = project();
      const payload = "private_native_file_bytes";
      writeFileSync(join(root, "source.txt"), payload);
      const adapter = join(root, "adapter.ts");
      const original: Record<string, string> = {
        code: 'code: "bad_option"', field: 'field: "future_face_budget"',
        requestId: 'requestId: "req-native-1"', operation: 'operation: call.request.operation as string',
      };
      const text = readFileSync(adapter, "utf8");
      writeFileSync(adapter, field === "taskId"
        ? text.replace('requestId: "req-native-1",', 'requestId: "req-native-1", taskId: text,')
        : text.replace(original[field]!, `${field}: text`));
      const wireField = field === "requestId" ? "request_id" : field === "taskId" ? "task_id" : field;
      for (const mode of ["reject", "uncertain"]) {
        const result = await invoke(root, mode, { mode });
        const diagnostic = result.steps["native#1"]?.diagnostic;
        expect(diagnostic).toBeDefined();
        expect((diagnostic as unknown as Record<string, unknown>)[wireField]).toBeUndefined();
        expect(diagnostic?.redacted).toBe(true);
        const record = await loadRun(result.runDir, { env });
        expect(JSON.stringify(await record.snapshot())).not.toContain(payload);
        expect(result.stdout).not.toContain(payload);
        const destination = join(root, `export-${mode}`);
        await record.export(destination, { includeDiagnostics: true });
        for (const path of files(destination).filter((path) => path.endsWith(".json"))) {
          expect(readFileSync(path, "utf8")).not.toContain(payload);
        }
        for (const path of files(join(root, ".fx/cache/jobs"))) {
          expect(readFileSync(path, "utf8")).not.toContain(payload);
        }
      }
    }
  }, SLOW * 3);
});
