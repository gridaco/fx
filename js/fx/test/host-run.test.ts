// The TypeScript node host's `run` (src/host.ts): outputs, the host's requests and their
// numbering, failures and their data, settling before the answer, one run at a time, `$/cancel`
// through `ctx.signal`, and the run-time closure guard. The cases follow
// python/tests/test_host_run.py where they apply.

import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import {
  builtHost,
  DEFER,
  expectValid,
  FakeEngine,
  HOST_SKIP,
  HOST_TEST,
  type HostProcess,
  makeProject,
  runParams,
  sha256,
  start,
  stopAll,
} from "./host-support.js";

afterEach(stopAll);

const BODIES = `import { writeFileSync } from "node:fs";
import { CallFailed, CeilingExceeded, EngineError, node } from "@grida/fx";
import { shout } from "./helper.ts";

export const writes = node(
  {
    name: "writes",
    inputs: { source: "text" },
    outputs: {
      text: "text",
      data: "json",
      copy: "text",
      file: "text/markdown",
      items: "text[]",
      keyed: "text{}",
      skipped: "text?",
    },
  },
  async (ctx) => {
    console.log("printed by the body");
    process.stdout.write("written to process.stdout by the body\\n");
    await ctx.fact("words", 2);
    await ctx.annotate({ shape: "box", box: [0, 0, 0.5, 0.5], label: "half" });
    ctx.progress("writing", 0.5);
    const note = ctx.out.path("note.md");
    writeFileSync(note, "# note\\n");
    return {
      text: ctx.out.text("hello"),
      data: ctx.out.json({ b: [1, 2], a: "é" }),
      copy: ctx.inputs.source,
      file: ctx.out.file(note),
      items: [ctx.out.text("one"), ctx.inputs.source],
      keyed: new Map([
        ["sun", ctx.out.text("sun")],
        ["10", ctx.out.text("ten")],
        ["2", ctx.out.text("two")],
      ]),
      skipped: null,
    };
  },
);

export const passes = node(
  { name: "passes", inputs: { badges: "image{}" }, outputs: { badges: "image{}", order: "json" } },
  (ctx) => ({ badges: ctx.inputs.badges, order: ctx.out.json([...ctx.inputs.badges.keys()]) }),
);

export const edits = node(
  { name: "edits", inputs: { image: "image" }, outputs: { image: "image" }, calls: { "image.edit": 1 } },
  async (ctx) => {
    const mask = ctx.out.bytes(new TextEncoder().encode("mask"), "image/png");
    const result = await ctx.imageEdit({ image: ctx.inputs.image, mask, prompt: "paint" });
    await ctx.fact("cached", result.cached);
    return { image: result.image };
  },
);

export const fails = node({ name: "fails", outputs: { x: "text" } }, async (ctx) => {
  await ctx.fact("kept", 1);
  await ctx.annotate({ label: "why" });
  throw ctx.fail("on purpose");
});

function helper(path: string): never {
  throw new TypeError("cannot read " + path);
}

export const crashes = node({ name: "crashes", outputs: { x: "text" } }, async (ctx) => {
  await ctx.fact("before", true);
  helper(new URL(import.meta.url).pathname);
});

export const syncCrashes = node({ name: "sync_crashes", outputs: { x: "text" } }, () => {
  throw new RangeError("k");
});

export const exits = node({ name: "exits", outputs: { x: "text" } }, () => {
  process.exit(3);
});

export const saysNothing = node({ name: "says_nothing", outputs: { x: "text" } }, () => {
  throw new Error();
});

export const throwsValue = node({ name: "throws_value", outputs: { x: "text" } }, () => {
  throw 42;
});

export const nanFact = node({ name: "nan_fact" }, async (ctx) => {
  await ctx.fact("score", 0 / 0);
});

export const badFacts = node({ name: "bad_facts" }, async (ctx) => {
  await ctx.annotate({ label: "kept" });
  (ctx.facts as Record<string, unknown>).broken = 0 / 0;
  throw ctx.fail("on purpose");
});

export const ceiling = node({ name: "ceiling", outputs: { image: "image" }, calls: { "image.generate": 1 } }, async (ctx) => {
  await ctx.imageGenerate({ prompt: "p" });
  return { image: null as never };
});

export const catches = node({ name: "catches", outputs: { image: "image" }, calls: { "image.generate": 1 } }, async (ctx) => {
  try {
    await ctx.imageGenerate({ prompt: "p" });
  } catch (error) {
    if (error instanceof CeilingExceeded) {
      throw ctx.fail("no money: " + error.message);
    }
    throw error;
  }
  return { image: null as never };
});

export const noImage = node({ name: "no_image", outputs: { image: "image" }, calls: { "image.generate": 1 } }, async (ctx) => {
  const result = await ctx.imageGenerate({ prompt: "p" });
  return { image: result.image };
});

export const oddCode = node({ name: "odd_code" }, () => {
  throw new EngineError("an odd code", null, -32050);
});

export const madeFailure = node({ name: "made_failure" }, () => {
  throw new CallFailed("the lighthouse fell");
});

export const notOutputs = node({ name: "not_outputs" }, () => [1] as never);
export const badOutput = node({ name: "bad_output", outputs: { x: "text" } }, () => ({ x: "plain string" as never }));
export const objectOnOne = node({ name: "object_on_one", outputs: { x: "text" } }, (ctx) => ({
  x: { a: ctx.out.text("a") } as never,
}));
export const objectOnKeyed = node({ name: "object_on_keyed", outputs: { x: "text{}" } }, (ctx) => ({
  x: { a: ctx.out.text("a") } as never,
}));
export const mapOnOne = node({ name: "map_on_one", outputs: { x: "text" } }, (ctx) => ({
  x: new Map([["a", ctx.out.text("a")]]) as never,
}));
export const numberKey = node({ name: "number_key", outputs: { x: "text{}" } }, (ctx) => ({
  x: new Map([[1, ctx.out.text("a")]]) as never,
}));
export const nothing = node({ name: "nothing", outputs: { x: "text?" } }, () => undefined);

export const waits = node({ name: "waits", outputs: { x: "text?" } }, async (ctx) => {
  ctx.progress("waiting");
  await new Promise((done) => {
    const timer = setTimeout(done, 20000);
    ctx.signal.addEventListener("abort", () => {
      clearTimeout(timer);
      done(undefined);
    });
  });
  await ctx.fact("cancelled", ctx.signal.aborted);
  await ctx.fact("reason", String((ctx.signal.reason as Error).message));
  return {};
});

export const rejectsOnAbort = node({ name: "rejects_on_abort", outputs: { x: "text?" } }, (ctx) =>
  new Promise((_, reject) => ctx.signal.addEventListener("abort", () => reject(ctx.signal.reason))),
);

export const leavesACall = node({ name: "leaves_a_call", calls: { "image.generate": 1 } }, (ctx) => {
  ctx.state.task = ctx.imageGenerate({ prompt: "later" });
  return {};
});

// A computed specifier: what it loads is known only once it runs.
export const lazy = node({ name: "lazy", outputs: { text: "text" } }, async (ctx) => {
  const name = "late";
  const { late } = await import(\`../lib/\${name}.ts\`);
  return { text: ctx.out.text(late) };
});

export const lazyThrows = node({ name: "lazy_throws" }, async () => {
  const name = "late";
  await import(\`../lib/\${name}.ts\`);
  throw new Error("thrown after a late import");
});

// A literal specifier counts whether or not it runs: lib/literal.ts is in this module's closure.
export const lazyLiteral = node({ name: "lazy_literal", outputs: { text: "text" } }, async (ctx) => {
  const { literal } = await import("../lib/literal.ts");
  return { text: ctx.out.text(literal) };
});

export const lazyKnown = node({ name: "lazy_known", outputs: { text: "text" } }, async (ctx) => {
  const helperModule = await import("./helper.ts");
  const path = await import("node:path");
  return { text: ctx.out.text(helperModule.shout(path.basename("a/b"))) };
});

export const quiet = node({ name: "quiet" }, () => ({}));

// Locating a file is not loading it.
export const locates = node({ name: "locates", outputs: { text: "text" } }, (ctx) => ({
  text: ctx.out.text(new URL(import.meta.resolve("../lib/asset.json")).pathname.endsWith("/lib/asset.json") ? "found" : "lost"),
}));

export const typed = node(
  {
    name: "typed",
    params: {
      shape: ["circle", "square"],
      size: "integer",
      note: { type: "string", optional: true },
      box: { type: "object", properties: { width: { type: "integer" } } },
      picture: { type: "string" },
    },
    outputs: { seen: "json" },
  },
  (ctx) => ({
    seen: ctx.out.json({
      keys: Object.keys(ctx.params).sort(),
      note: ctx.params.note ?? "absent",
      picture: typeof ctx.params.picture,
    }),
  }),
);

export const refs = node(
  {
    name: "refs",
    params: {
      refs: { type: "array", uniqueItems: true },
      strength: { type: "number", minimum: 0, maximum: 1, multipleOf: 0.1, default: 0.7 },
      code: { type: "string", pattern: "^\\\\d{3}\\\\-\\\\d{4}$" },
      note: { type: "string", optional: true, default: "none" },
    },
    outputs: { seen: "json" },
  },
  (ctx) => ({
    seen: ctx.out.json({
      refs: ctx.params.refs.length,
      strength: ctx.params.strength,
      code: ctx.params.code,
      note: ctx.params.note ?? "absent",
    }),
  }),
);

// Names the host's own package, as a plain path and as a URL.
export const namesTheSdk = node({ name: "names_the_sdk" }, () => {
  const url = import.meta.resolve("@grida/fx");
  throw new Error("at " + new URL(url).pathname + " and " + url);
});

export const sized = node(
  { name: "sized", params: { size: { type: "string", optional: true } }, calls: { "image.generate": 1 } },
  async (ctx) => {
    await ctx.imageGenerate({ prompt: "a lantern", size: ctx.params.size });
    await ctx.annotate({ label: "a", tag: undefined });
  },
);

export const leavesAProgram = node({ name: "leaves_a_program", tools: ["sh"] }, (ctx) => {
  void ctx.tool("sh").run(["-c", "sleep 30 & echo $! > " + ctx.workDir + "/sleeper.pid; wait"], { check: false });
  return new Promise((done) => setTimeout(() => done({}), 500));
});
`;

const REGISTRY = {
  // A shared registry that loads plugins lazily, when it is called.
  "lib/registry.ts": "export const load = (name: string) => import(`./plugins/${name}.ts`);\n",
  "lib/plugins/x.ts": 'export const x = "x";\n',
  "lib/plugins/y.ts": 'export const y = "y";\n',
  // Its top level calls the registry: plugins/x.ts is its own.
  "nodes/top.ts": `import { node } from "@grida/fx";
import { load } from "../lib/registry.ts";
const { x } = await load("x");
export const top = node({ name: "top", outputs: { text: "text" } }, (ctx) => ({ text: ctx.out.text(x) }));
`,
  // It only imports the registry.
  "nodes/user.ts": `import { node } from "@grida/fx";
import { load } from "../lib/registry.ts";
export const plain = node({ name: "plain" }, () => ({}));
export const loads = node({ name: "loads" }, async () => {
  await load("y");
});
`,
};

const PROJECT = {
  "nodes/bodies.ts": BODIES,
  // Imports lib/late.ts statically: once it ran, the file is loaded in the host.
  "nodes/preloads.ts": `import { node } from "@grida/fx";
import { late } from "../lib/late.ts";
export const preloads = node({ name: "preloads", outputs: { text: "text" } }, (ctx) => ({ text: ctx.out.text(late) }));
`,
  "nodes/helper.ts": "export const shout = (text: string): string => text.toUpperCase();\n",
  "lib/late.ts": 'export const late = "loaded late";\n',
  "lib/literal.ts": 'export const literal = "loaded by a literal import";\n',
  "lib/asset.json": "{}\n",
  ...REGISTRY,
};

/** A host over a new project, initialized, answered by a fake engine. */
async function started(): Promise<{ host: HostProcess; engine: FakeEngine; root: string }> {
  const root = makeProject(PROJECT);
  const host = start(root);
  const engine = new FakeEngine();
  host.engine = engine.answering;
  await host.initialize();
  return { host, engine, root };
}

/** Runs `attribute` of nodes/bodies.ts and returns its answer, held to the schema. */
async function run(host: HostProcess, engine: FakeEngine, attribute: string, over: Record<string, unknown> = {}) {
  const params = runParams({ path: "nodes/bodies.ts", attribute }, { work_dir: engine.newWorkDir(), ...over });
  const answer = await host.request("run", params);
  if (answer.result !== undefined) {
    expectValid(answer.result, "run_result");
  }
  return answer;
}

const methods = (host: HostProcess) => host.asked.filter((message) => message.id !== undefined).map((message) => message.method);

describe.skipIf(HOST_SKIP !== null)("the TypeScript node host's runs", () => {
  test(
    "a body writes its outputs: work paths, puts, passed-through files, lists and a Map in its order",
    async () => {
      const { host, engine } = await started();
      const source = engine.ref("source text", "text/plain", "source.txt");
      const answer = await run(host, engine, "writes", { inputs: { source } });
      expect(methods(host)).toEqual(["fact", "annotate", "file.put", "file.put", "file.put", "file.put", "file.put", "file.put"]);
      const [fact, annotate, ...puts] = host.asked.filter((message) => message.id !== undefined).map((message) => message.params as Record<string, unknown>);
      expect(fact).toEqual({ run_id: "r1", name: "words", value: 2 });
      expect(annotate).toEqual({ run_id: "r1", mark: { shape: "box", label: "half", box: [0, 0, 0.5, 0.5] } });
      expect(host.asked.filter((message) => message.id === undefined)).toEqual([
        { jsonrpc: "2.0", method: "progress", params: { run_id: "r1", text: "writing", fraction: 0.5 } },
      ]);
      expect(puts.map((put) => [put.name, put.kind])).toEqual([
        ["writes/text", "text/plain"],
        ["writes/data", "json"],
        ["writes/items[0]", "text/plain"],
        ["writes/keyed[sun]", "text/plain"],
        ["writes/keyed[10]", "text/plain"],
        ["writes/keyed[2]", "text/plain"],
      ]);
      expect(puts[1]?.json).toEqual({ b: [1, 2], a: "é" });
      const file = (data: string, kind: string, name: string) => ({ file: engine.ref(data, kind, name) });
      // The answer carries outputs only: facts and marks went with fact and annotate.
      expect(answer.result).toEqual({
        outputs: {
          text: file("hello", "text/plain", "writes/text"),
          data: file(JSON.stringify({ b: [1, 2], a: "é" }), "json", "writes/data"),
          copy: { file: source },
          file: { work_path: "out/note.md", kind: "text/markdown" },
          items: { list: [file("one", "text/plain", "writes/items[0]"), { file: source }] },
          keyed: {
            collection: [
              ["sun", file("sun", "text/plain", "writes/keyed[sun]")],
              ["10", file("ten", "text/plain", "writes/keyed[10]")],
              ["2", file("two", "text/plain", "writes/keyed[2]")],
            ],
          },
          skipped: null,
        },
      });
      expect(readFileSync(join(engine.workDir, "out", "note.md"), "utf8")).toBe("# note\n");
      expect(host.stderr).toContain("printed by the body");
      expect(host.stderr).toContain("written to process.stdout by the body");
    },
    HOST_TEST,
  );

  test(
    "a keyed input arrives as a Map in collection order and passes through",
    async () => {
      const { host, engine } = await started();
      const refs = ["sun", "10", "2"].map((key) => [key, engine.ref(`badge ${key}`, "image/png", `${key}.png`, key)] as const);
      const answer = await run(host, engine, "passes", { inputs: { badges: { collection: refs } } });
      const outputs = (answer.result as { outputs: Record<string, unknown> }).outputs;
      expect(outputs.badges).toEqual({ collection: refs.map(([key, ref]) => [key, { file: ref }]) });
      const order = host.asked.find((message) => message.method === "file.put")?.params as { json: unknown };
      expect(order.json).toEqual(["sun", "10", "2"]);
    },
    HOST_TEST,
  );

  test(
    "an async body calls a capability; host requests are numbered from 1",
    async () => {
      const { host, engine } = await started();
      const image = engine.ref("input image", "image/png", "in.png");
      const made = engine.ref("edited image", "image/png", "out.png");
      const calls: unknown[] = [];
      engine.answers.set("capability", (message) => {
        calls.push(message.params);
        expectValid(message.params, "capability_params");
        return { result: { key: "c".repeat(64), cached: true, cost_usd: 0, files: { image: made }, data: {} } };
      });
      const answer = await run(host, engine, "edits", { inputs: { image } });
      expect(answer.result).toEqual({ outputs: { image: { file: made } } });
      expect(methods(host)).toEqual(["file.put", "capability", "fact"]);
      expect((host.asked[0]?.params as { name: string }).name).toBe("request/file");
      expect(calls).toEqual([
        {
          run_id: "r1",
          capability: "image.edit",
          request: { image: { file: image.digest }, mask: { file: sha256("mask") }, prompt: "paint" },
        },
      ]);
      expect(host.asked[2]?.params).toEqual({ run_id: "r1", name: "cached", value: true });
      expect(host.asked.map((message) => message.id)).toEqual([1, 2, 3]);
    },
    HOST_TEST,
  );

  test(
    "a run is answered only after its own requests are",
    async () => {
      const { host, engine } = await started();
      engine.answers.set("capability", () => DEFER);
      const id = host.ask("run", runParams({ path: "nodes/bodies.ts", attribute: "leavesACall" }, { work_dir: engine.newWorkDir() }));
      const call = await host.next();
      expect(call.method).toBe("capability");
      // The body has returned, but its call is still pending: the host says nothing.
      await new Promise((done) => setTimeout(done, 300));
      expect(host.pending()).toBe(0);
      host.send({ jsonrpc: "2.0", id: call.id, result: { key: "c".repeat(64), cached: false, cost_usd: 0.04, files: {}, data: null } });
      const answer = await host.answerTo(id);
      expect(answer.result).toEqual({ outputs: {} });
    },
    HOST_TEST,
  );

  test(
    "node_failure keeps the body's facts and marks",
    async () => {
      const { host, engine } = await started();
      expect((await run(host, engine, "fails")).error).toEqual({
        code: -32000,
        message: "on purpose",
        data: { facts: { kept: 1 }, marks: [{ label: "why" }] },
      });
      // Facts the body broke by hand are left out.
      expect((await run(host, engine, "badFacts", { run_id: "r2" })).error).toEqual({
        code: -32000,
        message: "on purpose",
        data: { marks: [{ label: "kept" }] },
      });
    },
    HOST_TEST,
  );

  test(
    "node_error carries the error's name and its stack from user code, the root cut",
    async () => {
      const { host, engine, root } = await started();
      const error = (await run(host, engine, "crashes")).error;
      expect(error?.code).toBe(-32001);
      // The message is the error's own: the engine names the failure `<exception>: <message>`.
      expect(error?.message).toBe("cannot read nodes/bodies.ts");
      const data = error?.data as { exception: string; traceback: string; facts: unknown; marks: unknown };
      expect(data.exception).toBe("TypeError");
      expect(data.facts).toEqual({ before: true });
      expect(data.marks).toEqual([]);
      const lines = data.traceback.split("\n");
      expect(lines[0]).toBe("TypeError: cannot read nodes/bodies.ts");
      expect(lines[1]).toMatch(/^ +at helper \(nodes\/bodies\.ts:\d+:\d+\)$/);
      expect(data.traceback).toContain("nodes/bodies.ts");
      expect(data.traceback).not.toContain(root);
      expect(data.traceback).not.toContain("node:internal");
      expect(data.traceback).not.toContain("host.js");

      for (const [attribute, message, exception] of [
        ["syncCrashes", "k", "RangeError"],
        ["exits", "process.exit(3) cannot end the node host", "Error"],
        ["saysNothing", "Error", "Error"],
        ["throwsValue", "42", "NonError"],
        ["nanFact", "fact score: NaN is not a JSON number", "TypeError"],
        ["oddCode", "an odd code", "EngineError"],
      ] as const) {
        const failed = (await run(host, engine, attribute, { run_id: `r-${attribute}` })).error;
        expect([failed?.code, failed?.message, (failed?.data as { exception: string }).exception]).toEqual([-32001, message, exception]);
      }
      // The host goes on.
      expect((await run(host, engine, "fails", { run_id: "r-last" })).error?.code).toBe(-32000);
    },
    HOST_TEST,
  );

  test(
    "an engine error passes through, or the body catches it",
    async () => {
      const { host, engine } = await started();
      const data = { capability: "image.generate", needed_usd: 0.04, remaining_usd: 0.01 };
      const message = "run ceiling reached: ceiling#1 needs up to $0.04 and $0.01 is left";
      engine.answers.set("capability", () => ({ error: { code: -32014, message, data } }));
      expect((await run(host, engine, "ceiling")).error).toEqual({ code: -32014, message, data });
      expect((await run(host, engine, "catches", { run_id: "r2" })).error).toEqual({
        code: -32000,
        message: `no money: ${message}`,
        data: { facts: {}, marks: [] },
      });
      // One the body made itself keeps its code; so does a call that returned no image.
      expect((await run(host, engine, "madeFailure", { run_id: "r3" })).error).toEqual({
        code: -32016,
        message: "the lighthouse fell",
      });
      engine.answers.set("capability", () => ({
        result: { key: "c".repeat(64), cached: false, cost_usd: 0, files: {}, data: null },
      }));
      expect((await run(host, engine, "noImage", { run_id: "r4" })).error).toEqual({
        code: -32016,
        message: "the call returned no image",
      });
      // A malformed error and an answer with neither are the host's own errors, as the body sees them.
      engine.answers.set("capability", () => ({ error: { code: "x", message: 1 } }));
      const malformed = (await run(host, engine, "ceiling", { run_id: "r5" })).error;
      expect([malformed?.code, malformed?.message]).toEqual([-32099, "the engine answered capability with a malformed error"]);
    },
    HOST_TEST,
  );

  test(
    "outputs a body cannot return",
    async () => {
      const { host, engine } = await started();
      for (const [attribute, message] of [
        ["notOutputs", "not_outputs returned an array, not its outputs"],
        ["badOutput", "output x is a string; use ctx.out to make it"],
        ["objectOnOne", "output x is an object; use ctx.out to make it"],
        ["objectOnKeyed", "output x is an object; a keyed output is a Map"],
        ["mapOnOne", "output x is a Map; use ctx.out to make it"],
        ["numberKey", "output x has the key 1, which is not a string"],
      ] as const) {
        const error = (await run(host, engine, attribute, { run_id: `r-${attribute}` })).error;
        expect([error?.code, error?.message]).toEqual([-32000, message]);
      }
      expect((await run(host, engine, "nothing", { run_id: "r-nothing" })).result).toEqual({ outputs: {} });
    },
    HOST_TEST,
  );

  test(
    "bodies that cannot load, and run params that are refused",
    async () => {
      const { host, engine } = await started();
      expect((await run(host, engine, "missing")).error).toEqual({
        code: -32004,
        message: "nodes/bodies.ts: missing is not an export declared with node()",
      });
      expect((await host.request("run", runParams({ path: "nodes/none.ts", attribute: "x" }, { work_dir: engine.newWorkDir() }))).error).toEqual({
        code: -32004,
        message: "no module file nodes/none.ts",
      });
      expect((await host.request("run", runParams({ builtin: "fx/image.resize@1" }, { work_dir: engine.newWorkDir() }))).error).toEqual({
        code: -32004,
        message: "@grida/fx has no body for fx/image.resize@1",
      });
      for (const [change, message] of [
        [{ body: { path: "../x.ts", attribute: "x" } }, "run's body path is a POSIX path relative to the project root, not '../x.ts'"],
        [{ body: { builtin: "fx/a@1", path: "x" } }, "run's body is {path, attribute} or {builtin}"],
        [{ body: { path: "nodes/bodies.ts", attribute: "" } }, "run's body attribute is a name"],
        [{ body: "x" }, "run's body is {path, attribute} or {builtin}"],
        [{ work_dir: "relative" }, "run's work_dir is an absolute path"],
        [{ instance: { id: "x" } }, "run's instance is {id, path, step, key, take}"],
        [{ instance: { id: "x", path: "x", step: "x", key: null, take: [] } }, "run's instance is {id, path, step, key, take}"],
        [{ instance: { id: "x", path: "x", step: "x", key: null, take: [true] } }, "run's instance is {id, path, step, key, take}"],
        [{ run_id: "" }, "run's run_id is a non-empty string"],
        [{ inputs: [] }, "run's inputs is a JSON object"],
        [{ param_files: null }, "run's param_files is a JSON object"],
        [{ timeout_s: "1" }, "run's timeout_s is a number of seconds or null"],
      ] as const) {
        const params = runParams({ path: "nodes/bodies.ts", attribute: "writes" }, { work_dir: engine.newWorkDir(), ...change });
        expect((await host.request("run", params)).error).toEqual({ code: -32602, message });
      }
    },
    HOST_TEST,
  );

  test(
    "one run at a time; $/cancel aborts ctx.signal and the body decides",
    async () => {
      const { host, engine } = await started();
      const id = host.ask("run", runParams({ path: "nodes/bodies.ts", attribute: "waits" }, { work_dir: engine.newWorkDir() }));
      const progress = await host.next();
      expect(progress.method).toBe("progress");
      for (const [method, params] of [
        ["run", runParams({ path: "nodes/bodies.ts", attribute: "writes" }, { run_id: "r2", work_dir: engine.workDir })],
        ["describe", { targets: [], builtins: false }],
        ["build", { path: "b.ts", function: "b", arguments: {}, cwd: engine.workDir }],
      ] as const) {
        expect((await host.request(method, params)).error).toEqual({
          code: -32600,
          message: `${method} came while a run is pending`,
        });
      }
      host.notify("$/cancel", { id: 999 }); // names nothing pending: ignored
      host.notify("$/cancel", { id });
      const answer = await host.answerTo(id);
      expect(answer.result).toEqual({ outputs: {} });
      expect(host.asked.filter((message) => message.method === "fact").map((message) => message.params)).toEqual([
        { run_id: "r1", name: "cancelled", value: true },
        { run_id: "r1", name: "reason", value: "the engine cancelled run" },
      ]);
      // A body that rejects with the signal's reason fails as a node_error.
      const second = host.ask("run", runParams({ path: "nodes/bodies.ts", attribute: "rejectsOnAbort" }, { run_id: "r3", work_dir: engine.newWorkDir() }));
      await new Promise((done) => setTimeout(done, 100));
      host.notify("$/cancel", { id: second });
      const rejected = await host.answerTo(second);
      expect([rejected.error?.code, rejected.error?.message]).toEqual([-32001, "the engine cancelled run"]);
      // The host takes the next run.
      expect((await run(host, engine, "fails", { run_id: "r4" })).error?.code).toBe(-32000);
    },
    HOST_TEST,
  );

  test(
    "a body that loads a source file at run time, outside its closure, fails the node",
    async () => {
      const { host, engine, root } = await started();
      // A pool host that never described the module: its closure when it loaded is the baseline.
      expect((await run(host, engine, "lazy")).error).toEqual({
        code: -32000,
        message: "the body loaded lib/late.ts at run time, outside its source closure: import it statically",
        data: { facts: {}, marks: [] },
      });
      // A later run that imports it again fails too.
      expect((await run(host, engine, "lazy", { run_id: "r2" })).error?.message).toBe(
        "the body loaded lib/late.ts at run time, outside its source closure: import it statically",
      );
      // One that does not is not failed by what the run before it loaded.
      expect((await run(host, engine, "quiet", { run_id: "r3" })).result).toEqual({ outputs: {} });
      // Locating a file is not loading it.
      expect((await run(host, engine, "locates", { run_id: "r4" })).result).toBeDefined();
      // Importing a file already in the closure, or a builtin, is fine.
      const other = start(root);
      const otherEngine = new FakeEngine();
      other.engine = otherEngine.answering;
      await other.initialize();
      const known = await run(other, otherEngine, "lazyKnown");
      expect(known.result).toBeDefined();
      // So is a literal import that only the body runs: the file is in the module's closure.
      expect((await run(other, otherEngine, "lazyLiteral", { run_id: "r3" })).result).toBeDefined();
      // The guard wins over what the body threw.
      expect((await run(other, otherEngine, "lazyThrows", { run_id: "r4" })).error?.message).toBe(
        "the body loaded lib/late.ts at run time, outside its source closure: import it statically",
      );
      expect(existsSync(join(root, "lib", "late.ts"))).toBe(true);
    },
    HOST_TEST,
  );

  test(
    "a body's late import fails the node even when another module had loaded the file",
    async () => {
      const { host, engine } = await started();
      // Loaded outside any run: lib/late.ts is in this host now, but not in nodes/bodies.ts's closure.
      const { modules } = await host.describe([{ path: "nodes/preloads.ts" }]);
      expect((modules[0]?.closure as { label: string }[]).map((entry) => entry.label)).toContain("lib/late.ts");
      expect((await run(host, engine, "lazy", { run_id: "r1" })).error?.message).toBe(
        "the body loaded lib/late.ts at run time, outside its source closure: import it statically",
      );
      expect((await run(host, engine, "quiet", { run_id: "r2" })).result).toEqual({ outputs: {} });
    },
    HOST_TEST,
  );

  test(
    "what a shared module loads when another module's top level calls it is that module's, not every importer's",
    async () => {
      const { host, engine } = await started();
      const runOf = async (path: string, attribute: string, runId: string) => {
        const params = runParams({ path, attribute }, { run_id: runId, work_dir: engine.newWorkDir() });
        return host.request("run", params);
      };
      expect((await runOf("nodes/user.ts", "plain", "r0")).result).toEqual({ outputs: {} });
      expect((await runOf("nodes/top.ts", "top", "r1")).result).toBeDefined();
      // nodes/top.ts made the registry load plugins/x.ts: nodes/user.ts's body still loads nothing.
      expect((await runOf("nodes/user.ts", "plain", "r2")).result).toEqual({ outputs: {} });
      expect((await runOf("nodes/user.ts", "loads", "r3")).error?.message).toBe(
        "the body loaded lib/plugins/y.ts at run time, outside its source closure: import it statically",
      );
      expect((await runOf("nodes/user.ts", "plain", "r4")).result).toEqual({ outputs: {} });
      // The closures describe reports: the same whatever the session loaded before.
      const { modules } = await host.describe([{ path: "nodes/user.ts" }, { path: "nodes/top.ts" }]);
      expect(modules.map((module) => (module.closure as { label: string }[]).map((entry) => entry.label))).toEqual([
        ["lib/registry.ts", "nodes/user.ts"],
        ["lib/plugins/x.ts", "lib/registry.ts", "nodes/top.ts"],
      ]);
    },
    HOST_TEST,
  );

  test(
    "params are checked against their schemas before the body runs; a null optional param is absent",
    async () => {
      const { host, engine } = await started();
      const typed = (params: Record<string, unknown>, over: Record<string, unknown> = {}) =>
        run(host, engine, "typed", { params, ...over });
      for (const [params, message] of [
        [{ shape: "triangle", size: 1 }, 'param shape: "triangle" is not one of ["circle","square"]'],
        [{ shape: "circle", size: "big" }, 'param size: "big" is not of type "integer"'],
        [{ shape: "circle", size: null }, 'param size: null is not of type "integer"'],
        [{ shape: "circle", size: 1, box: { width: 1.5 } }, 'param box/width: 1.5 is not of type "integer"'],
      ] as const) {
        expect((await typed(params)).error).toEqual({ code: -32000, message, data: { facts: {}, marks: [] } });
      }
      expect(host.asked).toEqual([]);
      const answer = await typed({ shape: "circle", size: 1, note: null, picture: null }, { param_files: { "/picture": engine.ref("p", "image/png", "p.png") } });
      const seen = host.asked.find((message) => message.method === "file.put")?.params as { json: unknown };
      expect(answer.result).toBeDefined();
      expect(seen.json).toEqual({ keys: ["picture", "shape", "size"], note: "absent", picture: "object" });
    },
    HOST_TEST,
  );

  test(
    "files compare by digest, decimal steps and hand-written patterns are met, a null optional param with a default is absent",
    async () => {
      const { host, engine } = await started();
      const [a, b] = [engine.ref("a", "image/png", "a.png"), engine.ref("b", "image/png", "b.png")];
      const refs = (files: Record<string, unknown>, params: Record<string, unknown> = {}) =>
        run(host, engine, "refs", {
          params: { refs: [null, null], strength: 0.7, code: "123-4567", note: null, ...params },
          param_files: files,
        });
      const answer = await refs({ "/refs/0": a, "/refs/1": b });
      expect(answer.result).toBeDefined();
      const seen = host.asked.find((message) => message.method === "file.put")?.params as { json: unknown };
      expect(seen.json).toEqual({ refs: 2, strength: 0.7, code: "123-4567", note: "absent" });
      const digest = String(a.digest);
      expect((await refs({ "/refs/0": a, "/refs/1": a })).error?.message).toBe(
        `param refs: [{"file":"${digest}"},{"file":"${digest}"}] has repeated items`,
      );
      expect((await refs({ "/refs/0": a, "/refs/1": b }, { strength: 0.75 })).error?.message).toBe(
        "param strength: 0.75 is not a multiple of 0.1",
      );
      expect((await refs({ "/refs/0": a, "/refs/1": b }, { code: "not a code" })).error?.message).toBe(
        'param code: "not a code" does not match the pattern "^\\\\d{3}\\\\-\\\\d{4}$"',
      );
    },
    HOST_TEST,
  );

  test(
    "the host's own folder is named @grida/fx in what it sends",
    async () => {
      const { host, engine } = await started();
      const answer = await run(host, engine, "namesTheSdk");
      expect(answer.error?.message).toBe("at @grida/fx/dist/index.js and @grida/fx/dist/index.js");
      expect(JSON.stringify(answer)).not.toContain(builtHost().packageFolder);
    },
    HOST_TEST,
  );

  test(
    "a member whose value is undefined is left out of what the host sends",
    async () => {
      const { host, engine } = await started();
      engine.answers.set("capability", () => ({ result: { key: "c".repeat(64), cached: true, cost_usd: null, files: {}, data: null } }));
      expect((await run(host, engine, "sized", { calls: { "image.generate": 1 } })).result).toEqual({ outputs: {} });
      expect(host.asked.filter((message) => message.id !== undefined).map((message) => message.params)).toEqual([
        { run_id: "r1", capability: "image.generate", request: { prompt: "a lantern" } },
        { run_id: "r1", mark: { label: "a" } },
      ]);
    },
    HOST_TEST,
  );

  test(
    "a program the body left running ends once the run is answered",
    async () => {
      const { host, engine } = await started();
      const work = engine.newWorkDir();
      const params = runParams({ path: "nodes/bodies.ts", attribute: "leavesAProgram" }, { work_dir: work, tools: { sh: "/bin/sh" } });
      expect((await host.request("run", params)).result).toEqual({ outputs: {} });
      const sleeper = Number(readFileSync(join(work, "sleeper.pid"), "utf8").trim());
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
    },
    HOST_TEST,
  );
});
