import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { FileAccessError, FxError, listFiles, loadRun, resolveFile, run, type FileReference, type FileTarget } from "../src/index.js";
import { HOST_SKIP, makeProject } from "./host-support.js";
import { SLOW, temporaryFolder, testEngine } from "./support.js";

const artifact: FileTarget = { kind: "artifact", artifact: { digest: "a".repeat(64), kind: "image/png", name: "picture.png", size: 12, url: null } };
const source: FileTarget = { kind: "source", source: "nodes/draw.py", symbol: "draw", recorded_digest: "b".repeat(64) };
const reference = (target: FileTarget): FileReference => ({ subject: { kind: "run", workflow: { id: "case" } }, target });
const LIST = { kind: "fx-files-v1", files: [
  { target: artifact, role: "artifact", instance_id: "draw#1", scope_id: "scope:child#1" },
  { target: source, role: "node", instance_id: "draw#1" },
] };

function fake() {
  const folder = temporaryFolder("file-access");
  const binary = join(folder.path, "fake-engine");
  const config = join(folder.path, "reply.json"), log = join(folder.path, "log.jsonl");
  writeFileSync(binary, `#!/usr/bin/env node
const fs = require("node:fs");
const args = process.argv.slice(2);
const reply = JSON.parse(fs.readFileSync(process.env.FAKE_FILE_REPLY,"utf8"));
fs.appendFileSync(process.env.FAKE_FILE_LOG, JSON.stringify(args)+"\\n");
const body = args[0] === "inspect" ? {run:{folder:"runs/pinned",workflow:"case",state:"succeeded",charged_usd:0,steps:[]}} : reply.body;
process.stdout.write(typeof body === "string" ? body : JSON.stringify(body));
process.exit(args[0] === "inspect" ? 0 : reply.status ?? 0);
`);
  chmodSync(binary, 0o755);
  const reply = (body: unknown, status = 0) => writeFileSync(config, JSON.stringify({ body, status }));
  reply(LIST);
  return {
    ...folder, reply,
    options: { cwd: folder.path, env: { GRIDA_FX_BIN: binary, FAKE_FILE_REPLY: config, FAKE_FILE_LOG: log } },
    log: (): string[][] => existsSync(log) ? readFileSync(log, "utf8").trim().split("\n").map((line) => JSON.parse(line)) : [],
  };
}

describe.skipIf(spawnSync("node", ["--version"]).status !== 0 || process.platform === "win32")("native file access transport", () => {
  test("lists saved plans and pins record lookups with explicit project/instance bindings", async () => {
    const engine = fake();
    try {
      expect(await listFiles("saved-plan.json", { ...engine.options, project: "project folder", instanceId: "draw#1" })).toEqual([
        { target: artifact, role: "artifact", instanceId: "draw#1", scopeId: "scope:child#1" },
        { target: source, role: "node", instanceId: "draw#1" },
      ]);
      expect(engine.log()[0]).toEqual(["files", "list", "saved-plan.json", "--json", `--project=${join(engine.path, "project folder")}`, "--instance=draw#1"]);
      const loading = new AbortController();
      const record = await loadRun("case", { ...engine.options, signal: loading.signal });
      loading.abort();
      record.inspection.run.folder = "runs/changed";
      await record.listFiles();
      expect(engine.log().at(-1)).toEqual(["files", "list", join(engine.path, "runs/pinned"), "--json"]);
    } finally { engine.cleanup(); }
  }, SLOW);

  test("resolves the captured target, preserving changed source and unavailable copies", async () => {
    const engine = fake();
    try {
      const record = await loadRun("case", engine.options);
      const local = join(engine.path, "nodes/draw.py");
      engine.reply({ kind: "fx-file-resolution-v1", target: source, local_path: local, source_match: "changed", reason: null });
      expect(await record.resolveFile(reference(source), { project: engine.path, sourceRoot: "source owner" })).toEqual({
        target: source, localPath: local, sourceMatch: "changed", reason: null,
      });
      const args = engine.log().at(-1)!;
      expect(args.slice(0, 4)).toEqual(["files", "resolve", record.runDir, "--reference"]);
      expect(JSON.parse(args[4]!)).toEqual(reference(source));
      expect(args.slice(5)).toEqual(["--json", `--project=${engine.path}`, `--source-root=${join(engine.path, "source owner")}`]);
      engine.reply({ kind: "fx-file-resolution-v1", target: artifact, local_path: null, source_match: null, reason: "Recorded bytes are unavailable." });
      expect((await resolveFile("saved-plan.json", reference(artifact), engine.options)).localPath).toBeNull();
    } finally { engine.cleanup(); }
  }, SLOW);

  test("preserves structured refusals and rejects malformed native documents", async () => {
    const engine = fake();
    try {
      const refusal = { kind: "fx-file-error-v1" as const, code: "future_refusal", message: "Unavailable.", future_field: true };
      engine.reply(refusal, 2);
      const error = await resolveFile("run", reference(artifact), engine.options).catch((caught: unknown) => caught);
      expect(error).toBeInstanceOf(FileAccessError);
      expect((error as FileAccessError).code).toBe("future_refusal");
      expect((error as FileAccessError).document).toEqual(refusal);
      for (const body of ["not JSON", {}, { kind: "fx-files-v1", files: [{ target: source, role: "node", instance_id: 3 }] }, { ...LIST, files: [{ target: { kind: "source", source: "/private/file.py" }, role: "source" }] }]) {
        engine.reply(body);
        await expect(listFiles("run", engine.options)).rejects.toBeInstanceOf(FxError);
      }
      const resolution = { kind: "fx-file-resolution-v1", target: source, local_path: join(engine.path, "node.py"), source_match: "unknown", reason: null };
      for (const change of [{ local_path: "relative.py" }, { source_match: false }, { reason: 1 }, { target: {} }, { local_path: undefined }]) {
        engine.reply({ ...resolution, ...change });
        await expect(resolveFile("run", reference(source), engine.options)).rejects.toBeInstanceOf(FxError);
      }
      const before = engine.log().length;
      await expect(resolveFile("run", { ...reference(source), target: { kind: "source", source: "../outside.py" } }, engine.options)).rejects.toBeInstanceOf(TypeError);
      await expect(listFiles("run", { ...engine.options, project: "" })).rejects.toBeInstanceOf(TypeError);
      expect(engine.log()).toHaveLength(before);
    } finally { engine.cleanup(); }
  }, SLOW);
});

const binary = testEngine();
describe.skipIf(binary === null || HOST_SKIP !== null)("file access against the real engine", () => {
  test("resolves recorded bytes and changed current source without importing author code", async () => {
    const project = makeProject({
      "fx.yaml": "fx: project/v1\n",
      "nodes/source.ts": 'import { node } from "@grida/fx";\nexport const make = node({ name: "make", outputs: { text: "text" }, version: 1 }, ctx => ({ text: ctx.out.text("original bytes") }));\n',
      "workflow.yaml": "fx: workflow/v1\nid: file-access\ntitle: File access\nsteps:\n  make:\n    uses: ./nodes/source.ts#make\noutputs:\n  result: ${{ steps.make.outputs.text }}\n",
    });
    try {
      const options = { cwd: project, env: {
        GRIDA_FX_BIN: binary!, GRIDA_FX_NETWORK: "off", GRIDA_FX_DISABLE_DOTENV: "1",
        GG_API_KEY: undefined, OPENAI_API_KEY: undefined, OPENROUTER_API_KEY: undefined,
        FAL_KEY: undefined, TRIPO_API_KEY: undefined, ELEVENLABS_API_KEY: undefined,
      } };
      const result = await run("workflow.yaml", { ...options, maxUsd: 0 });
      expect(result.ok).toBe(true);
      const record = await loadRun(result.runDir, options);
      const files = await record.listFiles();
      const output = files.find((file) => file.target.kind === "artifact")!;
      const saved = await record.resolveFile({ subject: { kind: "run", workflow: { id: "file-access" } }, target: output.target });
      expect(saved.localPath).not.toBeNull();
      expect(readFileSync(saved.localPath!, "utf8")).toBe("original bytes");
      const node = files.find((file) => file.target.kind === "source" && file.target.source === "nodes/source.ts")!;
      writeFileSync(join(project, "nodes/source.ts"), 'throw new Error("source lookup must not import this");\n');
      const changed = await record.resolveFile({ subject: { kind: "run", workflow: { id: "file-access" } }, target: node.target }, { project });
      expect(changed.localPath).toBe(realpathSync(join(project, "nodes/source.ts")));
      expect(changed.sourceMatch).toBe("changed");
    } finally { rmSync(project, { recursive: true, force: true }); }
  }, SLOW);
});
