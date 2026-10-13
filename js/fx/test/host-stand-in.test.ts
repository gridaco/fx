// The TypeScript node host as a stand-in host (src/host.ts `serveStandIns`; spec/protocol.md
// §5.7): `stand_in.load` of a file outside the project, its failures, `stand_in.answer` one call at
// a time, faults and `$/cancel`. The cases follow python/tests/test_host.py's stand-in tests.

import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  expectValid,
  HOST_SKIP,
  HOST_TEST,
  type HostProcess,
  type Message,
  makeProject,
  start,
  stopAll,
} from "./host-support.js";

afterEach(stopAll);

const STAND_IN = `import { Answer, CallFailed, CallRefused, DECLINE, type StandInCall } from "@grida/fx";
import { PICTURE } from "./picture.ts";

let count = 0;

export async function answer(call: StandInCall) {
  count += 1;
  const prompt = String(call.request.prompt);
  if (prompt === "slow") {
    await new Promise((done) => setTimeout(done, 300));
    return Answer.json({ slow: true });
  }
  if (prompt === "decline") {
    return DECLINE;
  }
  if (prompt === "refuse") {
    throw new CallRefused("no lighthouses here");
  }
  if (prompt === "fail") {
    throw new CallFailed("the lighthouse fell");
  }
  if (prompt === "count") {
    return Answer.json({ asked: count });
  }
  if (prompt === "unsendable") {
    throw new CallRefused("no file gull\\udcff.png");
  }
  if (prompt === "boom") {
    throw new Error("a broken stand-in");
  }
  if (prompt === "wait") {
    await new Promise((done) => call.signal.addEventListener("abort", done));
    return DECLINE;
  }
  return { files: { image: PICTURE }, data: call.take > 1 ? { seen: call.take } : null };
}

export const NOT_A_FUNCTION = 3;
`;

const PICTURE = `export const PICTURE = new TextEncoder().encode("stand-in picture");
`;

/** A folder of stand-in files outside the project (and outside anything that installs
 * \`@grida/fx\`): the host's own package answers its imports of it. */
function standIns(): string {
  const folder = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-ts-stand-ins-")));
  process.on("exit", () => rmSync(folder, { recursive: true, force: true }));
  const files: Record<string, string> = {
    "package.json": JSON.stringify({ type: "module" }),
    "stand_in.ts": STAND_IN,
    "picture.ts": PICTURE,
    "broken.ts": 'throw new Error("stand-in dependencies missing");\n',
    "notes.txt": "not a module\n",
    "cjs/package.json": JSON.stringify({ type: "commonjs" }),
    "cjs/stand_in.js": "exports.answer = () => null;\n",
  };
  for (const [name, text] of Object.entries(files)) {
    mkdirSync(join(folder, name, ".."), { recursive: true });
    writeFileSync(join(folder, name), text);
  }
  return folder;
}

function ask(prompt: string, take: number[] = [1]): Record<string, unknown> {
  return {
    capability: "image.generate",
    route: { id: "img-a@acme", fingerprint: "e".repeat(64) },
    request: { prompt, size: "64x64" },
    take,
    key: "a".repeat(64),
    instance: { id: "draw#1", path: "draw", step: "draw" },
    files: {},
  };
}

async function loaded(host: HostProcess, path: string, fn = "answer"): Promise<void> {
  await host.initialize();
  const answer = await host.request("stand_in.load", { path, function: fn });
  expect(answer.result).toEqual({});
  expectValid(answer.result, "stand_in_load_result");
}

function answered(answer: Message): unknown {
  if (answer.result !== undefined) {
    expectValid(answer.result, "stand_in_answer_result");
    return answer.result;
  }
  return answer.error;
}

const PICTURE_BASE64 = Buffer.from("stand-in picture").toString("base64");
const CANCELLED = { code: -32002, message: "the engine cancelled stand_in.answer" };

describe.skipIf(HOST_SKIP !== null)("the TypeScript stand-in host", () => {
  test(
    "loads a file outside the project, then answers for it and nothing else",
    async () => {
      const project = makeProject({});
      const folder = standIns();
      const host = start(project);
      await loaded(host, join(folder, "stand_in.ts"));
      // Its sibling module loaded, and @grida/fx came from the host's own package.
      expect(answered(await host.request("stand_in.answer", ask("a lighthouse")))).toEqual({
        files: { image: { base64: PICTURE_BASE64 } },
        data: null,
      });
      const refused = { code: -32600, message: "this host answers for a stand-in" };
      expect((await host.request("describe", { targets: [], builtins: false })).error).toEqual(refused);
      expect((await host.build("build")).error).toEqual(refused);
      expect((await host.request("run", {})).error).toEqual(refused);
      expect((await host.request("stand_in.load", { path: join(folder, "stand_in.ts"), function: "answer" })).error).toEqual({
        code: -32600,
        message: "a stand-in is already loaded",
      });
      expect(await host.shutdown()).toBe(0);
    },
    HOST_TEST,
  );

  test(
    "load failures, named relative to the working directory when inside it",
    async () => {
      const project = makeProject({ "checks/local.ts": "export function answer() { return null; }\n" });
      const folder = standIns();
      const host = start(project);
      await host.initialize();
      expect((await host.request("stand_in.answer", ask("a lighthouse"))).error).toEqual({
        code: -32600,
        message: "stand_in.answer came before stand_in.load",
      });
      const load = async (path: string, fn = "answer") => (await host.request("stand_in.load", { path, function: fn })).error;
      expect(await load(join(folder, "missing.ts"))).toEqual({ code: -32004, message: `no stand-in file ${join(folder, "missing.ts")}` });
      expect(await load(join(folder, "stand_in.ts"), "nope")).toEqual({
        code: -32004,
        message: `${join(folder, "stand_in.ts")} has no function nope`,
      });
      expect((await load(join(folder, "stand_in.ts"), "NOT_A_FUNCTION"))?.message).toEndWith("has no function NOT_A_FUNCTION");
      expect(await load(join(folder, "broken.ts"))).toEqual({
        code: -32004,
        message: `${join(folder, "broken.ts")} failed to import: Error: stand-in dependencies missing`,
      });
      expect(await load(join(folder, "notes.txt"))).toEqual({ code: -32004, message: `cannot import ${join(folder, "notes.txt")}` });
      const cjs = join(folder, "cjs", "stand_in.js");
      expect(await load(cjs)).toEqual({
        code: -32004,
        message: `${cjs} failed to import: ${cjs} is not an ES module: set "type": "module" in package.json`,
      });
      // A file inside the working directory (the engine's) is named relative to it.
      expect(await load(join(project, "checks", "local.ts"), "nope")).toEqual({
        code: -32004,
        message: "checks/local.ts has no function nope",
      });
      expect(await load("relative.ts")).toEqual({
        code: -32602,
        message: "stand_in.load's path is the stand-in file's absolute path",
      });
      expect(await load(join(folder, "stand_in.ts"), "not a name")).toEqual({
        code: -32602,
        message: "stand_in.load's function is a name",
      });
      // A failed load leaves the host free to load another.
      expect((await host.request("stand_in.load", { path: join(folder, "stand_in.ts"), function: "answer" })).result).toEqual({});
    },
    HOST_TEST,
  );

  test(
    "answers one call at a time, in order; a fault is answered and every later call refused",
    async () => {
      const host = start(makeProject({}));
      await loaded(host, join(standIns(), "stand_in.ts"));
      const prompts = ["slow", "decline", "refuse", "fail", "count"];
      const ids = prompts.map((prompt) => host.ask("stand_in.answer", ask(prompt)));
      const answers: Message[] = [];
      for (let index = 0; index < prompts.length; index += 1) {
        answers.push(await host.next());
      }
      expect(answers.map((answer) => answer.id)).toEqual(ids);
      expect(answers.map(answered)).toEqual([
        { files: {}, data: { json: { slow: true } } },
        { decline: true },
        { code: -32015, message: "no lighthouses here" },
        { code: -32016, message: "the lighthouse fell" },
        { files: {}, data: { json: { asked: 5 } } },
      ]);
      // A message holding a lone surrogate still goes out, the surrogate written as its escape.
      expect((await host.request("stand_in.answer", ask("unsendable"))).error).toEqual({
        code: -32015,
        message: "no file gull\\udcff.png",
      });
      expect((await host.request("stand_in.answer", { ...ask("x"), take: [] })).error).toEqual({
        code: -32602,
        message: "stand_in.answer's take is a list of take numbers",
      });
      expect((await host.request("stand_in.answer", "params")).error).toEqual({
        code: -32602,
        message: "this method's params are a JSON object",
      });
      // A fault: answered internal, logged with its stack, and every later call refused.
      expect((await host.request("stand_in.answer", ask("boom"))).error).toEqual({
        code: -32099,
        message: "Error: a broken stand-in",
      });
      expect((await host.request("stand_in.answer", ask("count"))).error).toEqual({
        code: -32099,
        message: "the stand-in failed earlier",
      });
      expect(await host.shutdown()).toBe(0);
      const log = host.stderr;
      expect(log).toContain("@grida/fx node host: the stand-in failed:\nError: a broken stand-in\n");
      expect(log).toContain("stand_in.ts:");
      expect(log).not.toContain("stand-in.js");
    },
    HOST_TEST,
  );

  test(
    "$/cancel answers a call cancelled at once, waiting or running",
    async () => {
      const folder = standIns();
      const host = start(makeProject({}));
      await loaded(host, join(folder, "stand_in.ts"));
      // A call running: its signal is aborted, and it is answered cancelled.
      const waiting = host.ask("stand_in.answer", ask("wait"));
      await new Promise((done) => setTimeout(done, 50));
      host.notify("$/cancel", { id: waiting });
      expect(await host.next()).toEqual({ jsonrpc: "2.0", id: waiting, error: CANCELLED });
      expect(answered(await host.request("stand_in.answer", ask("a lighthouse", [1, 2])))).toEqual({
        files: { image: { base64: PICTURE_BASE64 } },
        data: { seen: 2 },
      });
      // A call waiting for its turn is cancelled at once; the running one finishes.
      const slow = host.ask("stand_in.answer", ask("slow"));
      const queued = host.ask("stand_in.answer", ask("decline"));
      host.notify("$/cancel", { id: queued });
      host.notify("$/cancel", { id: 999 }); // names nothing: ignored
      expect(await host.next()).toEqual({ jsonrpc: "2.0", id: queued, error: CANCELLED });
      expect(answered(await host.answerTo(slow))).toEqual({ files: {}, data: { json: { slow: true } } });
      await new Promise((done) => setTimeout(done, 100));
      expect(host.pending()).toBe(0);
    },
    HOST_TEST,
  );
});
