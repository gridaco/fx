// What the TypeScript node host's tests share: the compiled host (dist built into a temporary
// node_modules/@grida/fx once per test process), a driver that speaks the protocol to a host
// process as the engine does and holds every message the host writes to the protocol schema, and
// a fake engine that answers the host's requests. The host needs Node 24 or later with
// module.registerHooks: without it the tests are skipped, with the reason.

import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  closeSync,
  constants,
  copyFileSync,
  createReadStream,
  createWriteStream,
  mkdirSync,
  mkdtempSync,
  openSync,
  type ReadStream,
  readFileSync,
  realpathSync,
  rmSync,
  type WriteStream,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { FrameReader, frame, PROTOCOL, parseMessage } from "../src/protocol.js";
import { protocolErrors } from "./json-schema.js";
import { packageRoot } from "./support.js";

/** Why the host's tests cannot run here, or null when they can. */
export const HOST_SKIP: string | null = (() => {
  if (process.platform === "win32") {
    return "the TypeScript node host needs a POSIX system";
  }
  const probe = spawnSync(
    "node",
    ["-e", "process.stdout.write(process.versions.node + ' ' + typeof require('node:module').registerHooks)"],
    { encoding: "utf8" },
  );
  if (probe.status !== 0) {
    return "no node on PATH";
  }
  const [version = "", hooks] = probe.stdout.trim().split(" ");
  if (Number(version.split(".")[0]) < 24 || hooks !== "function") {
    return `node is ${version}; the TypeScript node host needs Node 24 or later with module.registerHooks`;
  }
  return null;
})();

if (HOST_SKIP !== null) {
  console.warn(`skipped: the TypeScript node host's tests (${HOST_SKIP})`);
}

/** A reader of its standard input that appends what it reads to the file `process.argv[1]`, at
 * most 16 KiB every 5 ms: slower than a body can print, as a terminal or the engine's copy can
 * be. */
const SLOW_READER = [
  'const fs = require("node:fs");',
  'const out = fs.openSync(process.argv[1], "a");',
  "const buffer = Buffer.alloc(16384);",
  "const pause = new Int32Array(new SharedArrayBuffer(4));",
  "for (;;) {",
  "  let count;",
  "  try {",
  "    count = fs.readSync(0, buffer, 0, buffer.length, null);",
  "  } catch (error) {",
  '    if (error.code !== "EAGAIN") throw error;',
  "    Atomics.wait(pause, 0, 0, 5);",
  "    continue;",
  "  }",
  "  if (count === 0) break;",
  "  fs.writeSync(out, buffer, 0, count);",
  "  Atomics.wait(pause, 0, 0, 5);",
  "}",
].join("\n");

/** The time a test that starts hosts may take. */
export const HOST_TEST = 60_000;
/** How long the driver waits for one message. */
const WAIT_MS = 20_000;

export interface BuiltHost {
  /** The folder holding `node_modules/@grida/fx` (projects made under it find it there). */
  readonly folder: string;
  /** `node_modules/@grida/fx`. */
  readonly packageFolder: string;
  /** `dist/host-entry.js`. */
  readonly entry: string;
}

let built: BuiltHost | null = null;

/** The host, compiled once per test process into a temporary `node_modules/@grida/fx`. */
export function builtHost(): BuiltHost {
  if (built !== null) {
    return built;
  }
  const folder = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-ts-host-")));
  process.on("exit", () => rmSync(folder, { recursive: true, force: true }));
  const packageFolder = join(folder, "node_modules", "@grida", "fx");
  mkdirSync(packageFolder, { recursive: true });
  copyFileSync(join(packageRoot, "package.json"), join(packageFolder, "package.json"));
  const tsc = join(packageRoot, "node_modules", "typescript", "bin", "tsc");
  const done = spawnSync("node", [tsc, "-p", "tsconfig.build.json", "--outDir", join(packageFolder, "dist")], {
    cwd: packageRoot,
    encoding: "utf8",
  });
  if (done.status !== 0) {
    throw new Error(`tsc failed:\n${done.stdout}${done.stderr}`);
  }
  built = { folder, packageFolder, entry: join(packageFolder, "dist", "host-entry.js") };
  return built;
}

let projects = 0;

/**
 * A new project of `files` (a `package.json` with `"type": "module"` unless given), under the
 * built host's folder, so `@grida/fx` resolves to it as an installed package; `outside` makes it
 * in a folder of its own instead, where nothing resolves `@grida/fx`. Returns its real path.
 */
export function makeProject(files: Record<string, string>, options: { outside?: boolean } = {}): string {
  projects += 1;
  let root: string;
  if (options.outside) {
    root = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-ts-project-")));
    process.on("exit", () => rmSync(root, { recursive: true, force: true }));
  } else {
    root = join(builtHost().folder, "projects", `p${projects}`);
    mkdirSync(root, { recursive: true });
  }
  const all = { "package.json": JSON.stringify({ type: "module" }), ...files };
  for (const [name, text] of Object.entries(all)) {
    const path = join(root, name);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, text);
  }
  return realpathSync(root);
}

/** A protocol message, as read. */
export type Message = Record<string, unknown> & {
  readonly id?: unknown;
  readonly method?: string;
  readonly params?: unknown;
  readonly result?: unknown;
  readonly error?: { readonly code: number; readonly message: string; readonly data?: unknown };
};

/** What a request handler returns to leave the request unanswered for now. */
export const DEFER = Symbol("defer");

/** Answers a host request: `{result}`, `{error}`, or {@link DEFER}. */
export type Answering = (message: Message) => { result: unknown } | { error: Record<string, unknown> } | typeof DEFER;

/**
 * A host process, driven frame by frame (module comment). Its protocol streams are named pipes, as
 * the engine's are pipes (Node and Bun give a child sockets, which Linux cannot open again through
 * /proc; on macOS the host reads a named pipe through the file system, so host.test.ts's
 * `PipedHost` gives one an anonymous pipe, as the engine does), and its stderr is a file the test
 * reads; `stderr: "pipe"` gives it a pipe of the test
 * runner's instead, and `stderr: "slow"` a pipe that a reader drains slowly into that file
 * ({@link SLOW_READER}; the child is then the shell that runs both).
 */
export class HostProcess {
  readonly child: ChildProcess;
  readonly root: string;
  /** Every host request and notification the driver saw while waiting for an answer. */
  readonly asked: Message[] = [];
  /** How the driver answers a host request while it waits (default: refused). */
  engine: Answering = (message) => ({
    error: { code: -32601, message: `the test engine has no method ${String(message.method)}` },
  });
  readonly #folder: string;
  readonly #log: string | null;
  #piped = "";
  readonly #input: WriteStream;
  readonly #output: ReadStream;
  #reader = new FrameReader();
  #messages: (Message | Error)[] = [];
  #waiting: ((value: Message | Error) => void) | null = null;
  #nextId = 1;
  readonly exited: Promise<{ code: number | null; signal: string | null }>;

  constructor(
    root: string,
    options: {
      env?: NodeJS.ProcessEnv;
      entry?: string;
      args?: string[];
      stderr?: "file" | "pipe" | "slow";
      detached?: boolean;
    } = {},
  ) {
    this.root = root;
    this.#folder = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-ts-pipes-")));
    const folder = this.#folder;
    process.on("exit", () => rmSync(folder, { recursive: true, force: true }));
    const input = join(folder, "in");
    const output = join(folder, "out");
    const made = spawnSync("mkfifo", [input, output], { encoding: "utf8" });
    if (made.status !== 0) {
      throw new Error(`mkfifo failed: ${made.stderr}`);
    }
    this.#log = options.stderr === "pipe" ? null : join(folder, "stderr.log");
    const stderr = this.#log === null ? "pipe" : openSync(this.#log, "a");
    const host = ["node", ...(options.args ?? []), options.entry ?? builtHost().entry];
    const script =
      options.stderr === "slow"
        ? [
            'in=$1; out=$2; log=$3; reader=$4; shift 4; "$@" <"$in" 2>&1 >"$out" | node -e "$reader" "$log"',
            "sh",
            input,
            output,
            this.#log as string,
            SLOW_READER,
          ]
        : ['in=$1; out=$2; shift 2; exec "$@" <"$in" >"$out"', "sh", input, output];
    this.child = spawn("/bin/sh", ["-c", ...script, ...host], {
      cwd: root,
      env: options.env ?? process.env,
      stdio: ["ignore", "ignore", stderr],
      detached: options.detached ?? false,
    });
    if (typeof stderr === "number") {
      closeSync(stderr);
    } else {
      this.child.stderr?.on("data", (chunk: Buffer) => {
        this.#piped += chunk.toString("utf8");
      });
    }
    this.#input = createWriteStream(input);
    this.#input.on("error", () => {
      // the host ended first
    });
    this.#output = createReadStream(output);
    this.#output.on("data", (chunk: Buffer | string) => {
      const bodies = this.#reader.push(typeof chunk === "string" ? Buffer.from(chunk) : chunk);
      if (this.#reader.broken !== null) {
        this.#push(new Error(`the protocol stream broke: ${this.#reader.broken.message}`));
      }
      for (const body of bodies) {
        let message: Message;
        try {
          message = parseMessage(body) as Message;
        } catch (error) {
          this.#push(new Error(`the host wrote what is not a message: ${(error as Error).message}`));
          continue;
        }
        const errors = protocolErrors(message);
        if (errors.length > 0) {
          this.#push(new Error(`the host wrote a message the schema refuses: ${JSON.stringify(message)}\n${errors.join("\n")}`));
          continue;
        }
        this.#push(message);
      }
    });
    this.#output.on("end", () => this.#push(new Error("the host closed its protocol stream")));
    this.#output.on("error", (error) => this.#push(error));
    this.exited = new Promise((done) => this.child.once("exit", (code, signal) => done({ code, signal })));
  }

  /** What the host wrote to stderr so far. */
  get stderr(): string {
    if (this.#log === null) {
      return this.#piped;
    }
    try {
      return readFileSync(this.#log, "utf8");
    } catch {
      return "";
    }
  }

  /** Ends the host's protocol input. */
  closeInput(): void {
    this.#input.end();
  }

  #push(item: Message | Error): void {
    const waiting = this.#waiting;
    if (waiting !== null) {
      this.#waiting = null;
      waiting(item);
    } else {
      this.#messages.push(item);
    }
  }

  /** The next message the host writes. */
  async next(timeoutMs = WAIT_MS): Promise<Message> {
    const queued = this.#messages.shift();
    const item =
      queued ??
      (await new Promise<Message | Error>((done) => {
        const timer = setTimeout(() => {
          this.#waiting = null;
          done(new Error(`no message from the host within ${timeoutMs} ms; its stderr:\n${this.stderr}`));
        }, timeoutMs);
        this.#waiting = (value) => {
          clearTimeout(timer);
          done(value);
        };
      }));
    if (item instanceof Error) {
      throw item;
    }
    return item;
  }

  /** Whether the host wrote a message the driver has not read. */
  pending(): number {
    return this.#messages.length;
  }

  sendRaw(data: Uint8Array | string): void {
    this.#input.write(data);
  }

  send(message: unknown): void {
    this.sendRaw(frame(message));
  }

  /** Sends a request; its id. */
  ask(method: string, params?: unknown): number {
    const id = this.#nextId;
    this.#nextId += 1;
    this.send({ jsonrpc: "2.0", id, method, ...(params === undefined ? {} : { params }) });
    return id;
  }

  /** Sends a request and waits for its answer, serving the host's own requests meanwhile. */
  async request(method: string, params?: unknown): Promise<Message> {
    return this.answerTo(this.ask(method, params));
  }

  /** Waits for the answer to the request `id`, serving the host's own requests meanwhile. */
  async answerTo(id: number | string, timeoutMs = WAIT_MS): Promise<Message> {
    for (;;) {
      const message = await this.next(timeoutMs);
      if (message.method === undefined && message.id === id) {
        return message;
      }
      this.serve(message);
    }
  }

  /** Records a host request or notification, and answers a request as {@link engine} says. */
  serve(message: Message): void {
    if (message.method === undefined) {
      throw new Error(`an answer to nothing the test asked: ${JSON.stringify(message)}`);
    }
    this.asked.push(message);
    if (message.id === undefined) {
      return;
    }
    const answer = this.engine(message);
    if (answer !== DEFER) {
      this.send({ jsonrpc: "2.0", id: message.id, ...answer });
    }
  }

  notify(method: string, params?: unknown): void {
    this.send({ jsonrpc: "2.0", method, ...(params === undefined ? {} : { params }) });
  }

  async initialize(sources: string[] = []): Promise<Record<string, unknown>> {
    const answer = await this.request("initialize", {
      protocol: PROTOCOL,
      engine: { name: "grida-fx", version: "0.1.0" },
      project_root: this.root,
      sources,
    });
    if (answer.result === undefined) {
      throw new Error(`initialize failed: ${JSON.stringify(answer)}`);
    }
    expectValid(answer.result, "initialize_result");
    return answer.result as Record<string, unknown>;
  }

  async describe(targets: Record<string, unknown>[], builtins = false): Promise<{
    modules: Record<string, unknown>[];
    builtins: unknown[];
  }> {
    const answer = await this.request("describe", { targets, builtins });
    if (answer.result === undefined) {
      throw new Error(`describe failed: ${JSON.stringify(answer)}`);
    }
    expectValid(answer.result, "describe_result");
    return answer.result as { modules: Record<string, unknown>[]; builtins: unknown[] };
  }

  async build(
    fn: string,
    options: { path?: string; cwd?: string; arguments?: Record<string, string> } = {},
  ): Promise<Message> {
    return this.request("build", {
      path: options.path ?? "builders/b.ts",
      function: fn,
      arguments: options.arguments ?? {},
      cwd: options.cwd ?? this.root,
    });
  }

  /** `shutdown`, then `exit`; the exit status. */
  async shutdown(): Promise<number | null> {
    const answer = await this.request("shutdown");
    if (answer.result !== null) {
      throw new Error(`shutdown answered ${JSON.stringify(answer)}`);
    }
    this.notify("exit");
    return (await this.ended()).code;
  }

  /** How the host process ended. */
  async ended(timeoutMs = WAIT_MS): Promise<{ code: number | null; signal: string | null }> {
    let timer: NodeJS.Timeout | undefined;
    const late = new Promise<never>((_, fail) => {
      timer = setTimeout(() => fail(new Error(`the host did not end within ${timeoutMs} ms`)), timeoutMs);
    });
    try {
      return await Promise.race([this.exited, late]);
    } finally {
      clearTimeout(timer);
    }
  }

  stop(): void {
    if (this.child.exitCode === null && this.child.signalCode === null) {
      this.child.kill("SIGKILL");
    }
    // Releases an open of either pipe still waiting for the other side.
    for (const [name, flags] of [
      ["in", constants.O_RDONLY | constants.O_NONBLOCK],
      ["out", constants.O_WRONLY | constants.O_NONBLOCK],
    ] as const) {
      try {
        closeSync(openSync(join(this.#folder, name), flags));
      } catch {
        // nobody waits
      }
    }
    this.#input.destroy();
    this.#output.destroy();
    this.child.stderr?.destroy();
  }
}

/** Throws unless `value` is valid as the protocol schema's `$defs/<name>`. */
export function expectValid(value: unknown, name: string): void {
  const errors = protocolErrors(value, name);
  if (errors.length > 0) {
    throw new Error(`not a valid ${name}: ${JSON.stringify(value)}\n${errors.join("\n")}`);
  }
}

/** Every host started by `start`, stopped by `stopAll` (call it in `afterEach`). */
const started: HostProcess[] = [];

export function start(root: string, options: ConstructorParameters<typeof HostProcess>[1] = {}): HostProcess {
  const host = new HostProcess(root, options);
  started.push(host);
  return host;
}

export function stopAll(): void {
  for (const host of started.splice(0)) {
    host.stop();
  }
}

/** A fake engine's store and its answers to the host's requests (`spec/protocol.md` §6). */
export class FakeEngine {
  readonly store: string;
  /** The run's work folder, for `file.put` by `work_path`. */
  workDir = "";
  /** Answers by method, over the defaults (`file.put`, `fact`, `annotate`, `prompt.render`). */
  readonly answers = new Map<string, Answering>();

  constructor() {
    this.store = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-ts-store-")));
    process.on("exit", () => rmSync(this.store, { recursive: true, force: true }));
  }

  /** A new work folder, which `workDir` becomes. */
  newWorkDir(): string {
    this.workDir = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-ts-work-")));
    const made = this.workDir;
    process.on("exit", () => rmSync(made, { recursive: true, force: true }));
    return made;
  }

  /** Stores `bytes` and returns their file ref. */
  ref(bytes: Uint8Array | string, kind: string, name = "", key?: string): Record<string, unknown> {
    const data = typeof bytes === "string" ? Buffer.from(bytes, "utf8") : Buffer.from(bytes);
    const digest = createHash("sha256").update(data).digest("hex");
    const path = join(this.store, digest);
    writeFileSync(path, data);
    return {
      digest,
      kind,
      size: data.length,
      name,
      ...(key === undefined ? {} : { key }),
      path,
      facts: { bytes: data.length, kind },
    };
  }

  /** The driver's {@link HostProcess.engine}. */
  readonly answering: Answering = (message) => {
    const method = String(message.method);
    const special = this.answers.get(method);
    if (special !== undefined) {
      return special(message);
    }
    const params = message.params as Record<string, unknown>;
    switch (method) {
      case "fact":
      case "annotate":
        return { result: {} };
      case "prompt.render":
        return { result: { text: `rendered ${String(params.path)}` } };
      case "file.put": {
        let data: Buffer;
        if (typeof params.work_path === "string") {
          data = readFileSync(join(this.workDir, params.work_path));
        } else if (typeof params.base64 === "string") {
          data = Buffer.from(params.base64, "base64");
        } else {
          data = Buffer.from(JSON.stringify(params.json), "utf8");
        }
        return { result: this.ref(data, String(params.kind ?? "file"), String(params.name ?? "")) };
      }
      default:
        return { error: { code: -32601, message: `the test engine has no method ${method}` } };
    }
  };
}

/** `run` params for `body`, with every field the engine sends. */
export function runParams(
  body: { path: string; attribute: string } | { builtin: string },
  over: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    run_id: "r1",
    instance: { id: "write#1", path: "writes", step: "writes", key: null, take: [1] },
    type: "0".repeat(64),
    body,
    params: {},
    param_files: {},
    inputs: {},
    work_dir: "/nonexistent",
    resources: {},
    tools: {},
    calls: {},
    timeout_s: null,
    ...over,
  };
}

/** sha256 of `text`, hex. */
export function sha256(text: string | Uint8Array): string {
  return createHash("sha256").update(text).digest("hex");
}
