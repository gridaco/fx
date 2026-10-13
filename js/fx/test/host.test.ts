// The TypeScript node host (src/host.ts, src/host-entry.ts), driven frame by frame as the engine
// drives it: the session, describe, build, keeping user output off the protocol, and the host's
// lifetime. Every message the host writes is held to the protocol schema. The cases follow
// python/tests/test_host.py where they apply to TypeScript modules.

import { afterEach, describe, expect, test } from "bun:test";
import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import {
  closeSync,
  constants,
  createWriteStream,
  existsSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readSync,
  realpathSync,
  rmSync,
  symlinkSync,
  unlinkSync,
  type WriteStream,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FrameReader, frame, PROTOCOL, parseMessage } from "../src/protocol.js";
import {
  builtHost,
  expectValid,
  FakeEngine,
  HOST_SKIP,
  HOST_TEST,
  makeProject,
  runParams,
  start,
  stopAll,
} from "./host-support.js";

afterEach(stopAll);

/** What descriptor 2 is in a Node process given a stderr the way the host's tests give one
 * (`stderr: "pipe"`), and whether that process can open `/dev/fd/2` again. */
async function stderrProbe(): Promise<{ socket: boolean; fifo: boolean; opens: boolean }> {
  const script = [
    'const fs = require("node:fs");',
    "const stat = fs.fstatSync(2);",
    "let opens = true;",
    "try { fs.closeSync(fs.openSync('/dev/fd/2', fs.constants.O_WRONLY | fs.constants.O_APPEND)); } catch { opens = false; }",
    "process.stdout.write(JSON.stringify({ socket: stat.isSocket(), fifo: stat.isFIFO(), opens }));",
  ].join("\n");
  const child = spawn("node", ["-e", script], { stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  child.stdout.on("data", (chunk: Buffer) => {
    out += chunk.toString("utf8");
  });
  child.stderr.resume();
  await new Promise((done) => child.once("close", done));
  return JSON.parse(out) as { socket: boolean; fifo: boolean; opens: boolean };
}

const N = `import { node } from "@grida/fx";
import { shout } from "./helper.ts";
import type { Shape } from "../lib/types.ts";

export const echo = node(
  { name: "echo", params: { text: "string" }, outputs: { text: "text" }, resources: ["prompts/r.md"] },
  (ctx) => ({ text: ctx.out.text(shout(ctx.params.text)) }),
);

export const pinned = node({ name: "pinned", outputs: { text: "text" }, version: 2 }, (ctx) => ({
  text: ctx.out.text("pinned"),
}));

export const helperValue: Shape = "round";
`;

const MANY = `import { node } from "@grida/fx";
import { echo } from "./n.ts";

export const zeta = node({ name: "zeta" }, () => ({}));
export const alpha = node({ name: "alpha", description: "The first letter." }, () => ({}));
export { echo as alias };
export const $odd = node({ name: "odd" }, () => ({}));
export default node({ name: "dflt" }, () => ({}));
`;

const COUNTED = `import { appendFileSync } from "node:fs";
import { node } from "@grida/fx";

appendFileSync(new URL("../loads.txt", import.meta.url), "loaded\\n");
export const counted = node({ name: "counted", version: 1 }, () => ({}));
`;

const FAILS_ONCE = `import { appendFileSync } from "node:fs";

appendFileSync(new URL("../failures.txt", import.meta.url), "failed\\n");
throw new Error("cannot start");
`;

const NOISY = `import { spawn, spawnSync } from "node:child_process";
import { readFileSync, writeSync } from "node:fs";
import { node } from "@grida/fx";

console.log("printed by console.log");
console.info("printed by console.info");
process.stdout.write("written to process.stdout\\n");
writeSync(1, "written to fd 1\\n");
spawnSync("sh", ["-c", "echo printed by a child"], { stdio: "inherit" });
await new Promise((done) =>
  spawn("sh", ["-c", "echo printed by an async child; sh -c 'echo printed by a grandchild'"], {
    stdio: "inherit",
  }).on("exit", done),
);
const stdin = readFileSync(0, "utf8");
const childStdin = spawnSync("sh", ["-c", "cat"], { stdio: ["inherit", "pipe", "inherit"], encoding: "utf8" }).stdout;
export const noisy = node(
  { name: "noisy", description: "stdin [" + stdin + "] child stdin [" + childStdin + "]" },
  () => ({}),
);
`;

const BUILDERS = `import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expr, workflow } from "@grida/fx";
import { made } from "./lib.ts";

export function build(args: Readonly<Record<string, string>>) {
  const text = readFileSync("level.txt", "utf8").trim();
  return workflow({
    id: "level-art",
    title: "Level " + args.level,
    steps: { draw: { uses: "./nodes/n.ts#echo", with: { text } } },
    outputs: { text: expr("steps.draw.outputs.text") },
  });
}

export async function later(args: Readonly<Record<string, string>>) {
  await new Promise((done) => setTimeout(done, 10));
  return build(args);
}

export function where() {
  return workflow({ id: "where", title: process.cwd(), steps: {} });
}

export function args(given: Readonly<Record<string, string>>) {
  return workflow({ id: "args", title: JSON.stringify([given, Object.isFrozen(given)]), steps: {} });
}

export const elsewhere = () => made();
export function boom() { throw new Error("no level"); }
export function crash() { throw new TypeError("broken builder"); }
export function exits() { process.exit(3); }
export function quiet() { throw new RangeError(); }
export function throwsValue() { throw "a string"; }
export function plain() { return { id: "x" }; }
export function text() { return "a workflow"; }
export function nothing() { return null; }
export function list() { return []; }
export function notJson() {
  return workflow({ id: "x", title: "x", steps: { a: { uses: "./n.ts#echo", with: { v: Number.NaN } } } });
}
export function undefinedMember() {
  return workflow({ id: "x", title: "x", description: undefined, steps: {} });
}
export function undefinedItem() {
  return workflow({ id: "x", title: "x", steps: { a: { uses: "./n.ts#echo", with: { v: [undefined] } } } });
}
export function missingFile() { return readFileSync(join(process.cwd(), "builders", "missing.txt")); }
export const NOT_A_FUNCTION = 1;
`;

const PROJECT = {
  "nodes/n.ts": N,
  "nodes/helper.ts": "export const shout = (text: string): string => text.toUpperCase();\n",
  "lib/types.ts": 'export type Shape = "round" | "square";\n',
  "nodes/many.ts": MANY,
  "nodes/counted.ts": COUNTED,
  "nodes/fails_once.ts": FAILS_ONCE,
  "nodes/noisy.ts": NOISY,
  "nodes/broken.ts": 'import { nope } from "./missing.ts";\nexport const broken = nope;\n',
  "nodes/syntax.ts": "export const a = 1;\nexport const b = ;\n",
  "nodes/syntax.js": "export const a = 1;\nexport const b = ;\n",
  "nodes/enum.ts": 'import { node } from "@grida/fx";\nexport enum Mode { A }\n',
  "nodes/bad_spec.ts": 'import { node } from "@grida/fx";\nexport const bad = node({ name: "Bad" }, () => ({}));\n',
  "nodes/named.ts": 'throw new Error("cannot start in " + import.meta.url + " or " + new URL(import.meta.url).pathname);\n',
  "nodes/value.ts": 'throw "a string";\n',
  "nodes/empty.ts": "throw new RangeError();\n",
  "nodes/exits.ts": "process.exit(3);\n",
  "nodes/declaration.d.ts": "export declare const x: number;\n",
  "nodes/late_error.ts": `import { node } from "@grida/fx";
setTimeout(() => { throw new Error("thrown later in " + import.meta.url); }, 0);
Promise.reject(new Error("rejected, unhandled"));
export const late = node({ name: "late" }, () => ({}));
`,
  "prompts/r.md": "a prompt\n",
  "builders/b.ts": BUILDERS,
  "builders/lib.ts": 'import { workflow } from "@grida/fx";\nexport const made = () => workflow({ id: "lib", title: "lib", steps: {} });\n',
  "builders/rows.ts": `import { readFileSync } from "node:fs";
import { workflow } from "@grida/fx";
const words = readFileSync("level.txt", "utf8").split(/\\s+/).filter(Boolean);
export function build() {
  return workflow({ id: "rows", title: "rows", steps: Object.fromEntries(words.map((w) => [w, { uses: "./nodes/n.ts#echo" }])) });
}
`,
  "builders/broken.ts": 'throw new Error("builder dependencies missing");\n',
};

function project(): string {
  return makeProject(PROJECT);
}

/** A work folder with `level.txt`, as Python's tests have. */
function workFolder(): string {
  const folder = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-ts-cwd-")));
  writeFileSync(join(folder, "level.txt"), "kitewharf\n");
  process.on("exit", () => rmSync(folder, { recursive: true, force: true }));
  return folder;
}

/**
 * A host whose protocol input is an anonymous pipe, as the engine's is: `cat` copies a named pipe
 * into it (the driver's pipes are named pipes, whose end of input kqueue never reports on macOS,
 * so there the host reads them through the file system). Its protocol output is a named pipe read
 * here without waiting, and its exit status is written to a file once it ends, since the shell
 * that runs the pipe outlives it while the input stays open.
 */
class PipedHost {
  readonly #folder = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-ts-pipes-")));
  readonly #child: ChildProcess;
  readonly #input: WriteStream;
  #output: number | null;
  readonly #frames = new FrameReader();
  readonly #root: string;

  constructor(root: string) {
    this.#root = root;
    const input = join(this.#folder, "in");
    const output = join(this.#folder, "out");
    if (spawnSync("mkfifo", [input, output]).status !== 0) {
      throw new Error("mkfifo failed");
    }
    this.#output = openSync(output, constants.O_RDONLY | constants.O_NONBLOCK);
    const stderr = openSync(join(this.#folder, "stderr.log"), "a");
    const script = 'in=$1; out=$2; status=$3; shift 3; cat "$in" | { "$@" >"$out"; echo $? >"$status"; }';
    const args = [input, output, join(this.#folder, "status"), "node", builtHost().entry];
    this.#child = spawn("/bin/sh", ["-c", script, "sh", ...args], { cwd: root, stdio: ["ignore", "ignore", stderr] });
    closeSync(stderr);
    this.#input = createWriteStream(input);
    this.#input.on("error", () => {
      // the host ended first
    });
  }

  get stderr(): string {
    return readFileSync(join(this.#folder, "stderr.log"), "utf8");
  }

  send(message: unknown): void {
    this.#input.write(frame(message));
  }

  /** Sends `initialize` and reads its answer: the host has its protocol streams then. */
  async initialize(): Promise<void> {
    this.send({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: { protocol: PROTOCOL, engine: { name: "grida-fx", version: "0.1.0" }, project_root: this.#root, sources: [] },
    });
    const chunk = Buffer.alloc(1 << 16);
    const deadline = Date.now() + 20_000;
    for (;;) {
      let count = 0;
      try {
        count = readSync(this.#output as number, chunk, 0, chunk.length, null);
      } catch (error) {
        if ((error as { code?: unknown }).code !== "EAGAIN") {
          throw error;
        }
      }
      const answers = count > 0 ? this.#frames.push(chunk.subarray(0, count)) : [];
      if (answers.length > 0) {
        expect((parseMessage(answers[0] as Uint8Array) as { id?: unknown }).id).toBe(1);
        return;
      }
      if (Date.now() > deadline) {
        throw new Error(`no answer to initialize; the host's stderr:\n${this.stderr}`);
      }
      await new Promise((done) => setTimeout(done, 10));
    }
  }

  /** The engine stops reading the protocol output. */
  closeOutput(): void {
    if (this.#output !== null) {
      closeSync(this.#output);
      this.#output = null;
    }
  }

  /** Ends the protocol input. */
  closeInput(): void {
    this.#input.end();
  }

  /** The host's exit status once it ends, or null when it has not ended within `timeoutMs`. */
  async status(timeoutMs: number): Promise<number | null> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      try {
        const text = readFileSync(join(this.#folder, "status"), "utf8");
        if (/^\d+\n$/.test(text)) {
          return Number(text.trim());
        }
      } catch {
        // not yet
      }
      await new Promise((done) => setTimeout(done, 20));
    }
    return null;
  }

  stop(): void {
    this.closeOutput();
    this.#input.destroy();
    if (this.#child.exitCode === null && this.#child.signalCode === null) {
      this.#child.kill("SIGKILL");
    }
    try {
      // Releases cat's open of the input if the driver never opened it.
      closeSync(openSync(join(this.#folder, "in"), constants.O_WRONLY | constants.O_NONBLOCK));
    } catch {
      // nobody waits
    }
    rmSync(this.#folder, { recursive: true, force: true });
  }
}

describe.skipIf(HOST_SKIP !== null)("the TypeScript node host", () => {
  describe("the session", () => {
    test(
      "initialize, shutdown, exit",
      async () => {
        const host = start(project());
        const result = await host.initialize();
        const manifest = JSON.parse(readFileSync(join(builtHost().packageFolder, "package.json"), "utf8"));
        const nodeVersion = spawnSync("node", ["-p", "process.versions.node"], { encoding: "utf8" }).stdout.trim();
        expect(result).toEqual({
          protocol: "fx-node-protocol-v1",
          host: { language: "typescript", version: nodeVersion, sdk_version: manifest.version },
        });
        const answer = await host.request("shutdown");
        expectValid(answer.result, "shutdown_result");
        expect(answer).toEqual({ jsonrpc: "2.0", id: 2, result: null });
        host.notify("exit");
        expect((await host.ended()).code).toBe(0);
        await expect(host.next()).rejects.toThrow("the host closed its protocol stream");
      },
      HOST_TEST,
    );

    test(
      "other header fields are ignored",
      async () => {
        const root = project();
        const host = start(root);
        const body = JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          method: "initialize",
          params: { protocol: PROTOCOL, engine: { name: "grida-fx", version: "0.1.0" }, project_root: root, sources: [] },
        });
        host.sendRaw(`Content-Length: ${Buffer.byteLength(body)}\r\nX-Other: 1\r\n\r\n${body}`);
        expect(((await host.next()).result as { protocol: string }).protocol).toBe(PROTOCOL);
      },
      HOST_TEST,
    );

    test(
      "a protocol mismatch leaves the session uninitialized",
      async () => {
        const root = project();
        const host = start(root);
        const manifest = JSON.parse(readFileSync(join(builtHost().packageFolder, "package.json"), "utf8"));
        const answer = await host.request("initialize", {
          protocol: "fx-node-protocol-v2",
          engine: { name: "grida-fx", version: "0.2.0" },
          project_root: root,
          sources: [],
        });
        expect(answer.error).toEqual({
          code: -32003,
          message: `the engine speaks fx-node-protocol-v2 and @grida/fx ${manifest.version} speaks fx-node-protocol-v1`,
          data: { engine_protocol: "fx-node-protocol-v2", host_protocol: "fx-node-protocol-v1" },
        });
        expect((await host.request("describe", { targets: [], builtins: false })).error?.code).toBe(-32600);
        host.notify("exit");
        expect((await host.ended()).code).toBe(1);
      },
      HOST_TEST,
    );

    test(
      "initialize's params are checked; a failed one can be tried again",
      async () => {
        const root = project();
        const host = start(root);
        const engine = { name: "grida-fx", version: "0.1.0" };
        for (const [params, message] of [
          [{ engine, project_root: root, sources: [] }, "initialize needs the engine's protocol"],
          [{ protocol: PROTOCOL, engine, project_root: "relative", sources: [] }, "project_root is the project's absolute path"],
          [{ protocol: PROTOCOL, engine, project_root: join(root, "nope"), sources: [] }, "project_root is not a folder"],
          [{ protocol: PROTOCOL, engine, project_root: join(root, "prompts", "r.md"), sources: [] }, "project_root is not a folder"],
          [{ protocol: PROTOCOL, engine, project_root: root, sources: ["Not An npm Name"] }, "sources is a list of package names"],
          [{ protocol: PROTOCOL, engine, project_root: root, sources: "acme" }, "sources is a list of package names"],
        ] as const) {
          expect((await host.request("initialize", params)).error).toEqual({ code: -32602, message });
        }
        expect((await host.request("initialize", [1])).error).toEqual({
          code: -32602,
          message: "this method's params are a JSON object",
        });
        await host.initialize(["acme_lib", "@acme/shapes", "left-pad"]);
      },
      HOST_TEST,
    );

    test(
      "requests before initialize, and initialize twice, are invalid",
      async () => {
        const host = start(project());
        for (const [method, params] of [
          ["describe", { targets: [], builtins: false }],
          ["shutdown", undefined],
          ["no.such.method", {}],
        ] as const) {
          const answer = await host.request(method, params);
          expect(answer.error).toEqual({ code: -32600, message: `${method} came before initialize` });
        }
        await host.initialize();
        expect((await host.request("initialize", { protocol: PROTOCOL })).error).toEqual({
          code: -32600,
          message: "initialize was already answered",
        });
      },
      HOST_TEST,
    );

    test(
      "unknown methods, and notifications that name nothing",
      async () => {
        const host = start(project());
        await host.initialize();
        for (const method of ["no.such.method", "capability", "file.put"]) {
          expect((await host.request(method, {})).error).toEqual({
            code: -32601,
            message: `the TypeScript node host has no method ${method}`,
          });
        }
        expect((await host.request("run", {})).error).toEqual({ code: -32602, message: "run's run_id is a non-empty string" });
        host.notify("$/cancel");
        host.notify("progress");
        host.notify("$/cancel", { id: 1 });
        host.notify("something.else", { x: 1 });
        expect((await host.request("shutdown")).result).toBeNull();
      },
      HOST_TEST,
    );

    test(
      "malformed messages",
      async () => {
        const host = start(project());
        await host.initialize();
        for (const body of [
          '{"jsonrpc": "2.0", "id": 9, "method": "describe", "params": {"x": NaN}}',
          '{"jsonrpc": "2.0", "id": 9, "method": "shutdown", "id": 9}',
          '{"jsonrpc": "2.0", "id": 9007199254740993, "method": "shutdown"}',
          '{"jsonrpc": "2.0", "id": 9, "method": "\\ud800"}',
          '{"jsonrpc": "2.0", "id": 9, "method": "shutdown", "params": 1e400}',
          Buffer.from([0xff]),
          "[1,",
        ]) {
          const bytes = typeof body === "string" ? Buffer.from(body) : body;
          host.sendRaw(Buffer.concat([Buffer.from(`Content-Length: ${bytes.length}\r\n\r\n`), bytes]));
          const answer = await host.next();
          expect(answer.id).toBeNull();
          expect(answer.error?.code).toBe(-32700);
        }
        for (const [message, code, text] of [
          [[1, 2], -32600, "a message is a JSON object"],
          [{ jsonrpc: "1.0", id: 3, method: "shutdown" }, -32600, 'a message carries "jsonrpc": "2.0"'],
          [{ jsonrpc: "2.0", id: true, method: "shutdown" }, -32600, "a request id is an integer or a string"],
          [{ jsonrpc: "2.0", id: null, method: "shutdown" }, -32600, "a request id is an integer or a string"],
          [{ jsonrpc: "2.0", id: 1.5, method: "shutdown" }, -32600, "a request id is an integer or a string"],
          [{ jsonrpc: "2.0", id: 4, method: 5 }, -32600, "a message's method is a string"],
          [{ jsonrpc: "2.0", id: 5, method: "describe", params: [1] }, -32602, "this method's params are a JSON object"],
          [{ jsonrpc: "2.0", id: 6, method: "describe", params: { targets: [] } }, -32602, "describe needs builtins: true or false"],
          [{ jsonrpc: "2.0", id: 6, method: "describe", params: { targets: {}, builtins: false } }, -32602, "describe needs targets: a list"],
          [{ jsonrpc: "2.0", id: 6, method: "describe", params: { targets: ["x"], builtins: false } }, -32602, "a describe target is {path, attribute?}"],
          [
            { jsonrpc: "2.0", id: 7, method: "describe", params: { targets: [{ path: "../x.ts" }], builtins: false } },
            -32602,
            "a describe target's path is a POSIX path relative to the project root, not '../x.ts'",
          ],
          [
            { jsonrpc: "2.0", id: 8, method: "describe", params: { targets: [{ path: "/abs/x.ts" }], builtins: false } },
            -32602,
            "a describe target's path is a POSIX path relative to the project root, not '/abs/x.ts'",
          ],
          [
            { jsonrpc: "2.0", id: 8, method: "describe", params: { targets: [{ path: "./x.ts" }], builtins: false } },
            -32602,
            "a describe target's path is a POSIX path relative to the project root, not './x.ts'",
          ],
          [
            { jsonrpc: "2.0", id: 8, method: "describe", params: { targets: [{ path: "x.ts", attribute: "" }], builtins: false } },
            -32602,
            "a describe target's attribute is a name",
          ],
          [{ jsonrpc: "2.0", id: "s", method: "shutdown", params: { now: true } }, -32602, "shutdown takes no params"],
        ] as const) {
          host.send(message);
          const answer = await host.next();
          expect(answer.error).toEqual({ code, message: text });
        }
        // A response the host never asked for is ignored; the session goes on.
        host.send({ jsonrpc: "2.0", id: 1, result: {} });
        expect((await host.request("shutdown")).result).toBeNull();
        host.notify("exit");
        expect((await host.ended()).code).toBe(0);
        expect(host.stderr).toContain("ignored a response to id 1");
      },
      HOST_TEST,
    );

    test(
      "a broken frame ends the session",
      async () => {
        const host = start(project());
        host.sendRaw("Content-Length: many\r\n\r\n{}");
        const answer = await host.next();
        expect(answer).toEqual({
          jsonrpc: "2.0",
          id: null,
          error: { code: -32700, message: "Content-Length is not a byte count: b' many'" },
        });
        expect((await host.ended()).code).toBe(1);
        // The frames before a broken one are answered.
        const root = project();
        const second = start(root);
        const body = Buffer.from(
          JSON.stringify({
            jsonrpc: "2.0",
            id: 1,
            method: "initialize",
            params: { protocol: PROTOCOL, engine: { name: "grida-fx", version: "0.1.0" }, project_root: root, sources: [] },
          }),
        );
        second.sendRaw(Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`), body, Buffer.from("no colon\r\n\r\n")]));
        expect(((await second.next()).result as { protocol: string }).protocol).toBe(PROTOCOL);
        expect((await second.next()).error).toEqual({ code: -32700, message: "a message header is not a field: b'no colon'" });
        expect((await second.ended()).code).toBe(1);
      },
      HOST_TEST,
    );

    test(
      "exit without shutdown, the end of input, and work after shutdown",
      async () => {
        const root = project();
        const host = start(root);
        await host.initialize();
        host.notify("exit");
        expect((await host.ended()).code).toBe(1);

        const ended = start(root);
        await ended.initialize();
        ended.closeInput();
        expect((await ended.ended()).code).toBe(1);

        const afterShutdown = start(root);
        await afterShutdown.initialize();
        await afterShutdown.request("shutdown");
        expect((await afterShutdown.request("describe", { targets: [], builtins: false })).error).toEqual({
          code: -32600,
          message: "describe came after shutdown",
        });
        expect((await afterShutdown.request("no.such.method")).error?.message).toBe("no.such.method came after shutdown");
        afterShutdown.closeInput();
        expect((await afterShutdown.ended()).code).toBe(0);

        // Frames after exit are never read.
        const last = start(root);
        await last.initialize();
        last.notify("exit");
        last.ask("describe", { targets: [], builtins: false });
        expect((await last.ended()).code).toBe(1);
        await expect(last.next()).rejects.toThrow("the host closed its protocol stream");
      },
      HOST_TEST,
    );

    test(
      "through an anonymous pipe, as the engine's: a failed write ends the host at once, and so does the end of input",
      async () => {
        const root = project();
        // The engine stops reading while it keeps the input open: the host's answer to describe
        // cannot be written, and it ends without waiting for more input.
        const failing = new PipedHost(root);
        try {
          await failing.initialize();
          failing.closeOutput();
          failing.send({ jsonrpc: "2.0", id: 2, method: "describe", params: { targets: [], builtins: false } });
          expect(await failing.status(5000)).toBe(1);
          expect(failing.stderr).toContain("could not write to the engine");
        } finally {
          failing.stop();
        }
        const ending = new PipedHost(root);
        try {
          await ending.initialize();
          ending.closeInput();
          expect(await ending.status(5000)).toBe(1);
        } finally {
          ending.stop();
        }
      },
      HOST_TEST,
    );

    test(
      "what user code leaves unhandled is logged, and the host goes on",
      async () => {
        const root = project();
        const host = start(root);
        await host.initialize();
        const { modules } = await host.describe([{ path: "nodes/late_error.ts" }]);
        expect((modules[0]?.types as { attribute: string }[]).map((type) => type.attribute)).toEqual(["late"]);
        await new Promise((done) => setTimeout(done, 200));
        expect(await host.shutdown()).toBe(0);
        expect(host.stderr).toContain("thrown later in nodes/late_error.ts");
        expect(host.stderr).toContain("rejected, unhandled");
        expect(host.stderr).not.toContain(root);
      },
      HOST_TEST,
    );
  });

  describe("describe", () => {
    test(
      "a good module: its types and its closure",
      async () => {
        const root = project();
        const host = start(root);
        await host.initialize();
        const result = await host.describe([{ path: "nodes/n.ts" }]);
        expect(result.builtins).toEqual([]);
        const [module] = result.modules;
        expect(module?.path).toBe("nodes/n.ts");
        const types = module?.types as { attribute: string; spec: Record<string, unknown> }[];
        expect(types.map((type) => type.attribute)).toEqual(["echo", "pinned"]);
        expect(types[0]?.spec).toEqual({
          name: "echo",
          inputs: {},
          params: { text: { type: "string" } },
          outputs: { text: "text" },
          judge: false,
          calls: {},
          resources: ["prompts/r.md"],
          tools: [],
          view: null,
          version: null,
          retry: "service",
        });
        expect(types[1]?.spec.version).toBe(2);
        // A type-only import is erased: lib/types.ts is not in the closure.
        expect(module?.closure).toEqual([
          { label: "nodes/helper.ts", path: join(root, "nodes", "helper.ts") },
          { label: "nodes/n.ts", path: join(root, "nodes", "n.ts") },
        ]);
      },
      HOST_TEST,
    );

    test(
      "one attribute, a missing one, and every export in name order",
      async () => {
        const host = start(project());
        await host.initialize();
        const result = await host.describe([
          { path: "nodes/n.ts", attribute: "pinned" },
          { path: "nodes/n.ts", attribute: "missing" },
          { path: "nodes/n.ts", attribute: "helperValue" },
          { path: "nodes/many.ts" },
          { path: "nodes/many.ts", attribute: "alias" },
          { path: "nodes/n.ts", attribute: null },
        ]);
        const [pinned, missing, helper, many, alias, all] = result.modules;
        expect((pinned?.types as { attribute: string }[]).map((type) => type.attribute)).toEqual(["pinned"]);
        expect(missing).toEqual({
          path: "nodes/n.ts",
          attribute: "missing",
          error: "nodes/n.ts: missing is not an export declared with node()",
        });
        expect(helper?.error).toBe("nodes/n.ts: helperValue is not an export declared with node()");
        // Exports in name order, an imported type's alias included; `default` and names that are
        // not plain identifiers are left out.
        const types = many?.types as { attribute: string; spec: Record<string, unknown> }[];
        expect(types.map((type) => type.attribute)).toEqual(["alias", "alpha", "zeta"]);
        expect(types[1]?.spec.description).toBe("The first letter.");
        expect((many?.closure as { label: string }[]).map((entry) => entry.label)).toEqual([
          "nodes/helper.ts",
          "nodes/many.ts",
          "nodes/n.ts",
        ]);
        expect((alias?.types as { spec: { name: string } }[])[0]?.spec.name).toBe("echo");
        expect((all?.types as unknown[]).length).toBe(2);
      },
      HOST_TEST,
    );

    test(
      "modules that fail name project files relatively",
      async () => {
        const root = project();
        const outside = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-ts-outside-")));
        process.on("exit", () => rmSync(outside, { recursive: true, force: true }));
        writeFileSync(join(outside, "elsewhere.ts"), "export const x = 1;\n");
        symlinkSync(join(outside, "elsewhere.ts"), join(root, "nodes", "link.ts"));
        const host = start(root);
        await host.initialize();
        const result = await host.describe([
          { path: "nodes/broken.ts" },
          { path: "nodes/broken.ts", attribute: "anything" },
          { path: "nodes/syntax.ts" },
          { path: "nodes/syntax.js" },
          { path: "nodes/enum.ts" },
          { path: "nodes/bad_spec.ts" },
          { path: "nodes/named.ts" },
          { path: "nodes/value.ts" },
          { path: "nodes/empty.ts" },
          { path: "nodes/exits.ts" },
          { path: "nodes/none.ts" },
          { path: "nodes/link.ts" },
          { path: "prompts/r.md" },
          { path: "nodes/declaration.d.ts" },
        ]);
        const errors = result.modules.map((entry) => [entry.attribute ?? null, entry.error]);
        expect(errors).toEqual([
          [null, "nodes/broken.ts failed to import: ERR_MODULE_NOT_FOUND: Cannot find module 'nodes/missing.ts' imported from nodes/broken.ts"],
          ["anything", "nodes/broken.ts failed to import: ERR_MODULE_NOT_FOUND: Cannot find module 'nodes/missing.ts' imported from nodes/broken.ts"],
          [null, "nodes/syntax.ts failed to import: ERR_INVALID_TYPESCRIPT_SYNTAX: Expression expected (nodes/syntax.ts, line 2)"],
          [null, "nodes/syntax.js failed to import: SyntaxError: Unexpected token ';'"],
          [
            null,
            "nodes/enum.ts failed to import: ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX: TypeScript enum is not supported in strip-only mode (nodes/enum.ts, line 2)",
          ],
          [null, "nodes/bad_spec.ts failed to import: SpecError: node type name 'Bad' must be lower_snake words joined by ."],
          [null, "nodes/named.ts failed to import: Error: cannot start in nodes/named.ts or nodes/named.ts"],
          [null, "nodes/value.ts failed to import: NonError: a string"],
          [null, "nodes/empty.ts failed to import: RangeError"],
          [null, "nodes/exits.ts failed to import: Error: process.exit(3) cannot end the node host"],
          [null, "no module file nodes/none.ts"],
          [null, "nodes/link.ts is outside the project"],
          [null, "cannot import prompts/r.md"],
          [null, "cannot import nodes/declaration.d.ts"],
        ]);
        for (const entry of result.modules) {
          expect(String(entry.error)).not.toContain(root);
          expect(entry.types).toBeUndefined();
        }
        // The host keeps serving.
        expect(await host.shutdown()).toBe(0);
      },
      HOST_TEST,
    );

    test(
      "every type is checked again, as the engine checks it: a spec another copy of @grida/fx made too",
      async () => {
        const spec = (name: string, inputs: Record<string, string>) =>
          JSON.stringify({ name, inputs, params: {}, outputs: {}, judge: false, calls: {}, resources: [], tools: [], view: null, version: null, retry: "service" });
        const root = makeProject({
          "nodes/foreign.ts": [
            `export const bad = { [Symbol.for("grida.fx.node")]: { spec: ${spec("bad", { Image: "image" })}, body() { return {}; } } };`,
            `export const good = { [Symbol.for("grida.fx.node")]: { spec: ${spec("good", { image: "image[]" })}, body() { return {}; } } };`,
            "",
          ].join("\n"),
        });
        const host = start(root);
        await host.initialize();
        const { modules } = await host.describe([{ path: "nodes/foreign.ts", attribute: "good" }, { path: "nodes/foreign.ts" }]);
        expect((modules[0]?.types as { spec: unknown }[])[0]?.spec).toEqual(JSON.parse(spec("good", { image: "image[]" })));
        expect(modules[1]).toEqual({
          path: "nodes/foreign.ts",
          error: "nodes/foreign.ts: bad: bad: input name 'Image' must be one lower_snake word (a letter, then letters, digits or _)",
        });
      },
      HOST_TEST,
    );

    test(
      "each module loads once per session, a failure too",
      async () => {
        const root = project();
        const host = start(root);
        await host.initialize();
        const first = await host.describe([{ path: "nodes/counted.ts" }, { path: "nodes/fails_once.ts" }]);
        const second = await host.describe([
          { path: "nodes/counted.ts", attribute: "counted" },
          { path: "nodes/fails_once.ts" },
        ]);
        expect(first.modules[0]?.types).toEqual(second.modules[0]?.types);
        expect(first.modules[1]?.error).toBe(second.modules[1]?.error);
        expect(first.modules[1]?.error).toBe("nodes/fails_once.ts failed to import: Error: cannot start");
        expect(readFileSync(join(root, "loads.txt"), "utf8")).toBe("loaded\n");
        expect(readFileSync(join(root, "failures.txt"), "utf8")).toBe("failed\n");
        // A new session loads again.
        const again = start(root);
        await again.initialize();
        await again.describe([{ path: "nodes/counted.ts" }]);
        expect(readFileSync(join(root, "loads.txt"), "utf8")).toBe("loaded\nloaded\n");
      },
      HOST_TEST,
    );

    test(
      "built-ins are the engine's",
      async () => {
        const host = start(project());
        await host.initialize();
        expect(await host.describe([], true)).toEqual({ modules: [], builtins: [] });
      },
      HOST_TEST,
    );
  });

  describe("user output stays off the protocol", () => {
    test(
      "console, process.stdout, descriptor 1, children and a grandchild write to stderr; stdin is empty",
      async () => {
        const host = start(project());
        await host.initialize();
        const { modules } = await host.describe([{ path: "nodes/noisy.ts" }]);
        const [type] = modules[0]?.types as { attribute: string; spec: { description: string } }[];
        expect(type?.attribute).toBe("noisy");
        // The module read its stdin, and so did a child: both got nothing.
        expect(type?.spec.description).toBe("stdin [] child stdin []");
        expect(await host.shutdown()).toBe(0);
        await expect(host.next()).rejects.toThrow("the host closed its protocol stream");
        for (const line of [
          "printed by console.log",
          "printed by console.info",
          "written to process.stdout",
          "written to fd 1",
          "printed by a child",
          "printed by an async child",
          "printed by a grandchild",
        ]) {
          expect(host.stderr).toContain(line);
        }
      },
      HOST_TEST,
    );

    test(
      "a program user code leaves running does not hold the protocol stream",
      async () => {
        const root = makeProject({
          "nodes/lingers.ts": `import { spawn, spawnSync } from "node:child_process";
import { node } from "@grida/fx";
spawn("sh", ["-c", "sleep 30"], { stdio: "inherit", detached: true }).unref();
export const lingers = node({ name: "lingers" }, () => ({}));
`,
        });
        const host = start(root);
        await host.initialize();
        await host.describe([{ path: "nodes/lingers.ts" }]);
        await host.request("shutdown");
        host.notify("exit");
        expect((await host.ended()).code).toBe(0);
        // The protocol stream ends with the host: the sleeping program never held it.
        const started = Date.now();
        await expect(host.next(10_000)).rejects.toThrow("the host closed its protocol stream");
        expect(Date.now() - started).toBeLessThan(5_000);
      },
      HOST_TEST,
    );

    test(
      "what a body prints reaches a slow stderr whole before the host ends",
      async () => {
        const root = makeProject({
          "nodes/loud.ts": `import { node } from "@grida/fx";
export const loud = node({ name: "loud" }, () => {
  const text = "x".repeat(96);
  for (let index = 0; index < 20000; index += 1) {
    console.error(\`line \${index} \${text}\`);
  }
  return {};
});
`,
        });
        // About 2 MB, printed far faster than the reader takes it.
        const host = start(root, { stderr: "slow" });
        const engine = new FakeEngine();
        host.engine = engine.answering;
        await host.initialize();
        const params = runParams({ path: "nodes/loud.ts", attribute: "loud" }, { work_dir: engine.newWorkDir() });
        expect((await host.request("run", params)).result).toEqual({ outputs: {} });
        expect(await host.shutdown()).toBe(0);
        const lines = host.stderr.split("\n").filter((line) => line.startsWith("line "));
        expect(lines.length).toBe(20000);
        expect(lines[19999]).toStartWith("line 19999 ");
      },
      HOST_TEST,
    );
  });

  describe("a stderr the host cannot open again", () => {
    test(
      "the host refuses to start; where it can open it, what user code prints reaches it",
      async () => {
        const root = makeProject({
          "nodes/talks.ts": `import { node } from "@grida/fx";
console.log("printed by console.log");
process.stdout.write("written to process.stdout\\n");
console.error("printed by console.error");
export const talks = node({ name: "talks" }, () => ({}));
`,
        });
        // The test runner's pipe: a socket, which Linux cannot open again through /proc (macOS can).
        // A probe given a stderr the same way says which this is.
        const probe = await stderrProbe();
        expect(probe.socket || probe.fifo).toBe(true);
        const host = start(root, { stderr: "pipe" });
        const stderrHas = async (lines: string[]) => {
          for (let tries = 0; tries < 40 && !lines.every((line) => host.stderr.includes(line)); tries += 1) {
            await new Promise((done) => setTimeout(done, 50));
          }
          for (const line of lines) {
            expect(host.stderr).toContain(line);
          }
        };
        if (!probe.opens) {
          expect((await host.ended()).code).toBe(70);
          await stderrHas(["@grida/fx node host: stderr could not become descriptor 1"]);
          return;
        }
        await host.initialize();
        const { modules } = await host.describe([{ path: "nodes/talks.ts" }]);
        expect((modules[0]?.types as { attribute: string }[]).map((type) => type.attribute)).toEqual(["talks"]);
        expect(await host.shutdown()).toBe(0);
        await stderrHas(["printed by console.log", "written to process.stdout", "printed by console.error"]);
      },
      HOST_TEST,
    );
  });

  describe("build", () => {
    test(
      "a builder's document, with its own path as the takes anchor",
      async () => {
        const root = project();
        const work = workFolder();
        const host = start(root);
        await host.initialize();
        const answer = await host.build("build", { cwd: work, arguments: { level: "one" } });
        expectValid(answer.result, "build_result");
        expect(answer.result).toEqual({
          document: {
            fx: "workflow/v1",
            id: "level-art",
            title: "Level one",
            steps: { draw: { uses: "./nodes/n.ts#echo", with: { text: "kitewharf" } } },
            outputs: { text: "${{ steps.draw.outputs.text }}" },
          },
          takes_anchor: "builders/b.ts",
        });
        // An async builder; arguments arrive as one frozen object; a workflow made in another
        // module still anchors to the builder file.
        expect(((await host.build("later", { cwd: work, arguments: { level: "two" } })).result as { document: { title: string } }).document.title).toBe("Level two");
        expect(((await host.build("args", { cwd: work, arguments: { b: "2", a: "1" } })).result as { document: { title: string } }).document.title).toBe(
          '[{"b":"2","a":"1"},true]',
        );
        expect(((await host.build("elsewhere", { cwd: work })).result as { takes_anchor: string }).takes_anchor).toBe("builders/b.ts");
      },
      HOST_TEST,
    );

    test(
      "a builder runs in cwd, which is restored after",
      async () => {
        const root = project();
        const work = workFolder();
        const host = start(root);
        await host.initialize();
        const where = await host.build("where", { cwd: work });
        expect(realpathSync((where.result as { document: { title: string } }).document.title)).toBe(work);
        // A relative read resolves against cwd, not the project root.
        expect((await host.build("build", { cwd: root, arguments: { level: "x" } })).error).toEqual({
          code: -32005,
          message: "build: Error: ENOENT: no such file or directory, open 'level.txt'",
        });
        // And the builder module's top-level code runs in cwd too.
        writeFileSync(join(work, "level.txt"), "alpha beta\n");
        writeFileSync(join(root, "level.txt"), "gamma\n");
        const rows = await host.build("build", { path: "builders/rows.ts", cwd: work });
        expect(Object.keys((rows.result as { document: { steps: object } }).document.steps)).toEqual(["alpha", "beta"]);
        unlinkSync(join(root, "level.txt"));
        const other = start(root);
        await other.initialize();
        expect((await other.build("build", { path: "builders/rows.ts", cwd: join(work, "..") })).error).toEqual({
          code: -32004,
          message: "builders/rows.ts failed to import: Error: ENOENT: no such file or directory, open 'level.txt'",
        });
        const restored = await other.build("where", { cwd: work });
        expect(realpathSync((restored.result as { document: { title: string } }).document.title)).toBe(work);
      },
      HOST_TEST,
    );

    test.each([
      ["boom", "builders/b.ts", {}, -32005, "boom: Error: no level"],
      ["crash", "builders/b.ts", {}, -32005, "crash: TypeError: broken builder"],
      ["exits", "builders/b.ts", {}, -32005, "exits: Error: process.exit(3) cannot end the node host"],
      ["quiet", "builders/b.ts", {}, -32005, "quiet: RangeError"],
      ["throwsValue", "builders/b.ts", {}, -32005, "throwsValue: NonError: a string"],
      ["plain", "builders/b.ts", {}, -32005, "plain returned an object, not a workflow document"],
      ["text", "builders/b.ts", {}, -32005, "text returned a string, not a workflow document"],
      ["nothing", "builders/b.ts", {}, -32005, "nothing returned null, not a workflow document"],
      ["list", "builders/b.ts", {}, -32005, "list returned an array, not a workflow document"],
      ["notJson", "builders/b.ts", {}, -32005, "notJson: TypeError: at /steps/a/with/v: NaN is not a JSON number"],
      ["undefinedItem", "builders/b.ts", {}, -32005, "undefinedItem: TypeError: at /steps/a/with/v/0: undefined is not a JSON value"],
      ["missingFile", "builders/b.ts", {}, -32005, "missingFile: Error: ENOENT: no such file or directory, open 'builders/missing.txt'"],
      ["nope", "builders/b.ts", {}, -32004, "builders/b.ts has no function nope"],
      ["NOT_A_FUNCTION", "builders/b.ts", {}, -32004, "builders/b.ts has no function NOT_A_FUNCTION"],
      ["build", "builders/none.ts", {}, -32004, "no builder file builders/none.ts"],
      ["build", "builders/broken.ts", {}, -32004, "builders/broken.ts failed to import: Error: builder dependencies missing"],
    ])(
      "the builder %s of %s fails",
      async (fn, path, args, code, message) => {
        const root = project();
        const work = workFolder();
        const host = start(root);
        await host.initialize();
        const answer = await host.build(fn, { path, cwd: fn === "missingFile" ? root : work, arguments: args });
        expect(answer.error).toEqual({ code, message });
        // The host keeps serving, in its own working directory.
        const where = await host.build("where", { cwd: work });
        expect(realpathSync((where.result as { document: { title: string } }).document.title)).toBe(work);
      },
      HOST_TEST,
    );

    test(
      "a member whose value is undefined is left out of the document, as toYaml leaves it out",
      async () => {
        const host = start(project());
        await host.initialize();
        const answer = await host.build("undefinedMember", { cwd: workFolder() });
        expectValid(answer.result, "build_result");
        const document = (answer.result as { document: Record<string, unknown> }).document;
        expect(document).toEqual({ fx: "workflow/v1", id: "x", title: "x", steps: {} });
        expect(Object.hasOwn(document, "description")).toBe(false);
      },
      HOST_TEST,
    );

    test(
      "build's params are checked",
      async () => {
        const root = project();
        const work = workFolder();
        const host = start(root);
        await host.initialize();
        for (const [params, message] of [
          [{ path: "builders/b.ts", function: "build", arguments: {}, cwd: "relative" }, "build's cwd is an absolute path"],
          [{ path: "builders/b.ts", function: "build", arguments: {} }, "build's cwd is an absolute path"],
          [{ path: "builders/b.ts", function: "a-b", arguments: {}, cwd: work }, "build's function is a name"],
          [{ path: "builders/b.ts", function: "build", arguments: { a: 1 }, cwd: work }, "build's arguments map names to strings"],
          [
            { path: "./builders/b.ts", function: "build", arguments: {}, cwd: work },
            "build's path is a POSIX path relative to the project root, not './builders/b.ts'",
          ],
          [{ path: "builders/b.ts", function: "where", arguments: {}, cwd: join(work, "no") }, "build's cwd is not a folder this host can enter"],
        ] as const) {
          expect((await host.request("build", params)).error).toEqual({ code: -32602, message });
        }
      },
      HOST_TEST,
    );
  });

  describe("the host's lifetime", () => {
    test(
      "process.exit belongs to the host",
      async () => {
        const host = start(project());
        await host.initialize();
        const { modules } = await host.describe([{ path: "nodes/exits.ts" }]);
        expect(modules[0]?.error).toBe("nodes/exits.ts failed to import: Error: process.exit(3) cannot end the node host");
        expect(await host.shutdown()).toBe(0);
      },
      HOST_TEST,
    );

    test(
      "a host whose engine is gone ends, with what it started",
      async () => {
        const root = makeProject({
          "nodes/busy.ts": `import { spawn, spawnSync } from "node:child_process";
import { writeFileSync } from "node:fs";
const child = spawn("sleep", ["60"], { stdio: "ignore" });
writeFileSync(new URL("../child.pid", import.meta.url), String(child.pid));
for (;;) {}
`,
        });
        // A stand-in engine: it starts the host as the leader of a group of its own, on named
        // pipes, sends initialize and describe, and then is killed.
        const engineScript = join(root, "engine.mjs");
        writeFileSync(
          engineScript,
          `import { spawn, spawnSync } from "node:child_process";
import { createReadStream, createWriteStream, openSync, writeFileSync } from "node:fs";
const [root, entry] = process.argv.slice(2);
const frame = (message) => { const body = Buffer.from(JSON.stringify(message)); return Buffer.concat([Buffer.from("Content-Length: " + body.length + "\\r\\n\\r\\n"), body]); };
spawnSync("mkfifo", [root + "/in", root + "/out"]);
const host = spawn("/bin/sh", ["-c", 'in=$1; out=$2; shift 2; exec "$@" <"$in" >"$out"', "sh", root + "/in", root + "/out", process.execPath, entry], { cwd: root, detached: true, stdio: ["ignore", "ignore", openSync(root + "/stderr.log", "a")] });
writeFileSync(root + "/host.pid", String(host.pid));
const input = createWriteStream(root + "/in");
createReadStream(root + "/out").resume();
input.write(frame({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocol: "fx-node-protocol-v1", engine: { name: "grida-fx", version: "0.1.0" }, project_root: root, sources: [] } }));
input.write(frame({ jsonrpc: "2.0", id: 2, method: "describe", params: { targets: [{ path: "nodes/busy.ts" }], builtins: false } }));
setInterval(() => {}, 1000);
`,
        );
        const engine = spawn("node", [engineScript, root, builtHost().entry], { stdio: "ignore" });
        const childPidFile = join(root, "child.pid");
        for (let tries = 0; tries < 200 && !existsSync(childPidFile); tries += 1) {
          await new Promise((done) => setTimeout(done, 50));
        }
        expect(existsSync(childPidFile)).toBe(true);
        const hostPid = Number(readFileSync(join(root, "host.pid"), "utf8"));
        const childPid = Number(readFileSync(childPidFile, "utf8"));
        const alive = (pid: number) => {
          try {
            process.kill(pid, 0);
            return true;
          } catch {
            return false;
          }
        };
        expect(alive(hostPid)).toBe(true);
        engine.kill("SIGKILL");
        const deadline = Date.now() + 10_000;
        while (Date.now() < deadline && (alive(hostPid) || alive(childPid))) {
          await new Promise((done) => setTimeout(done, 100));
        }
        expect(alive(hostPid)).toBe(false);
        expect(alive(childPid)).toBe(false);
        const engineErr = readFileSync(join(root, "stderr.log"), "utf8");
        expect(engineErr).toContain("the engine that started this host is gone; ending");
      },
      HOST_TEST,
    );

    test(
      "an exit listener that throws does not keep the host running",
      async () => {
        const root = makeProject({
          "nodes/holds.ts": `import { node } from "@grida/fx";
process.on("exit", () => {
  throw new Error("an exit listener that throws");
});
setInterval(() => {}, 1000);
export const holds = node({ name: "holds" }, () => ({}));
`,
        });
        const host = start(root);
        await host.initialize();
        await host.describe([{ path: "nodes/holds.ts" }]);
        expect(await host.shutdown()).toBe(0);
      },
      HOST_TEST,
    );

    test(
      "the programs a pending run started end with the host",
      async () => {
        const root = makeProject({
          "nodes/runs.ts": `import { node } from "@grida/fx";
export const runs = node({ name: "runs", tools: ["sh"] }, async (ctx) => {
  await ctx.tool("sh").run(["-c", "sleep 60 & echo $! > " + ctx.workDir + "/tool.pid; wait"]);
});
`,
        });
        const work = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-ts-work-")));
        const host = start(root);
        await host.initialize();
        host.ask("run", {
          run_id: "r1",
          instance: { id: "runs#1", path: "runs", step: "runs", key: null, take: [1] },
          type: "0".repeat(64),
          body: { path: "nodes/runs.ts", attribute: "runs" },
          params: {},
          param_files: {},
          inputs: {},
          work_dir: work,
          resources: {},
          tools: { sh: "/bin/sh" },
          calls: {},
          timeout_s: null,
        });
        const pidFile = join(work, "tool.pid");
        for (let tries = 0; tries < 200 && !(existsSync(pidFile) && readFileSync(pidFile, "utf8").trim()); tries += 1) {
          await new Promise((done) => setTimeout(done, 50));
        }
        const sleeper = Number(readFileSync(pidFile, "utf8").trim());
        host.closeInput();
        expect((await host.ended()).code).toBe(1);
        let alive = true;
        for (let tries = 0; tries < 100 && alive; tries += 1) {
          try {
            process.kill(sleeper, 0);
            await new Promise((done) => setTimeout(done, 50));
          } catch {
            alive = false;
          }
        }
        expect(alive).toBe(false);
        rmSync(work, { recursive: true, force: true });
      },
      HOST_TEST,
    );

    test(
      "a host whose engine is gone ends the programs a body started too",
      async () => {
        const root = makeProject({
          "nodes/blocks.ts": `import { node } from "@grida/fx";
export const blocks = node({ name: "blocks", tools: ["sh"] }, (ctx) => {
  void ctx.tool("sh").run(["-c", "sleep 60 & echo $! > " + ctx.workDir + "/tool.pid; wait"]);
  for (;;) {}
});
`,
        });
        const work = join(root, "work");
        const engineScript = join(root, "engine.mjs");
        writeFileSync(
          engineScript,
          `import { spawn, spawnSync } from "node:child_process";
import { createReadStream, createWriteStream, mkdirSync, openSync, writeFileSync } from "node:fs";
const [root, entry, work] = process.argv.slice(2);
mkdirSync(work, { recursive: true });
const frame = (message) => { const body = Buffer.from(JSON.stringify(message)); return Buffer.concat([Buffer.from("Content-Length: " + body.length + "\\r\\n\\r\\n"), body]); };
spawnSync("mkfifo", [root + "/in", root + "/out"]);
const host = spawn("/bin/sh", ["-c", 'in=$1; out=$2; shift 2; exec "$@" <"$in" >"$out"', "sh", root + "/in", root + "/out", process.execPath, entry], { cwd: root, detached: true, stdio: ["ignore", "ignore", openSync(root + "/stderr.log", "a")] });
writeFileSync(root + "/host.pid", String(host.pid));
const input = createWriteStream(root + "/in");
createReadStream(root + "/out").resume();
input.write(frame({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocol: "fx-node-protocol-v1", engine: { name: "grida-fx", version: "0.1.0" }, project_root: root, sources: [] } }));
input.write(frame({ jsonrpc: "2.0", id: 2, method: "run", params: { run_id: "r1", instance: { id: "blocks#1", path: "blocks", step: "blocks", key: null, take: [1] }, type: "0".repeat(64), body: { path: "nodes/blocks.ts", attribute: "blocks" }, params: {}, param_files: {}, inputs: {}, work_dir: work, resources: {}, tools: { sh: "/bin/sh" }, calls: {}, timeout_s: null } }));
setInterval(() => {}, 1000);
`,
        );
        const engine = spawn("node", [engineScript, root, builtHost().entry, work], { stdio: "ignore" });
        const pidFile = join(work, "tool.pid");
        for (let tries = 0; tries < 200 && !(existsSync(pidFile) && readFileSync(pidFile, "utf8").trim()); tries += 1) {
          await new Promise((done) => setTimeout(done, 50));
        }
        const sleeper = Number(readFileSync(pidFile, "utf8").trim());
        const hostPid = Number(readFileSync(join(root, "host.pid"), "utf8"));
        const alive = (pid: number) => {
          try {
            process.kill(pid, 0);
            return true;
          } catch {
            return false;
          }
        };
        expect(alive(sleeper)).toBe(true);
        engine.kill("SIGKILL");
        const deadline = Date.now() + 10_000;
        while (Date.now() < deadline && (alive(hostPid) || alive(sleeper))) {
          await new Promise((done) => setTimeout(done, 100));
        }
        expect(alive(hostPid)).toBe(false);
        expect(alive(sleeper)).toBe(false);
      },
      HOST_TEST,
    );
  });
});
