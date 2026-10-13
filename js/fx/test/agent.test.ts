// Agents and their tools (src/ctx.ts `AgentLoop`, src/node.ts `tool`) over a fake channel to the
// engine (spec/protocol.md §5.4, §5.5, §6.2): the cases of python/tests/test_agent.py that apply to
// TypeScript bodies.

import { afterAll, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { type AgentLoop, type Channel, RunContext, type RunParams } from "../src/ctx.js";
import { AgentUnfinished, EngineError, engineError, NodeFailure } from "../src/engine-errors.js";
import { InputFile } from "../src/files.js";
import { SUBMIT, type Tool, ToolReply, tool } from "../src/node.js";
import { checkValue } from "../src/protocol.js";

const folder = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-agent-")));
afterAll(() => rmSync(folder, { recursive: true, force: true }));
let works = 0;

const sha256 = (data: string | Uint8Array) => createHash("sha256").update(data).digest("hex");
const base64 = (data: string) => Buffer.from(data, "utf8").toString("base64");
const PNG_URL = `data:image/png;base64,${base64("png bytes")}`;

/** Records requests; `file.put` stores into a folder; `agent.run` is answered by a function given
 * the params (it may call back into the agent, as the engine does). */
class FakeChannel implements Channel {
  readonly requests: [string, Record<string, unknown>][] = [];
  agentRun: ((params: Record<string, unknown>) => Promise<unknown>) | null = null;
  readonly store: string;
  readonly workDir: string;

  constructor(store: string, workDir: string) {
    this.store = store;
    this.workDir = workDir;
  }

  async request(method: string, params: Record<string, unknown>): Promise<unknown> {
    try {
      checkValue({ run_id: "r1", ...params }, "", 1);
    } catch (error) {
      throw new TypeError(`${method}: ${(error as Error).message}`);
    }
    this.requests.push([method, JSON.parse(JSON.stringify(params))]);
    if (method === "agent.run") {
      if (this.agentRun === null) {
        throw new Error("no agent.run answer");
      }
      return this.agentRun(params);
    }
    if (method === "file.put") {
      const data =
        typeof params.base64 === "string"
          ? Buffer.from(params.base64, "base64")
          : typeof params.work_path === "string"
            ? readFileSync(join(this.workDir, params.work_path))
            : Buffer.from(JSON.stringify(params.json));
      const digest = sha256(data);
      mkdirSync(this.store, { recursive: true });
      writeFileSync(join(this.store, digest), data);
      const kind = String(params.kind ?? "file");
      return { digest, kind, size: data.length, name: String(params.name ?? "f"), path: join(this.store, digest), facts: { bytes: data.length, kind } };
    }
    if (method === "fact" || method === "annotate") {
      return {};
    }
    throw new Error(`unexpected request ${method}`);
  }

  notify(method: string): void {
    throw new Error(method);
  }

  puts(): Record<string, unknown>[] {
    return this.requests.filter(([method]) => method === "file.put").map(([, params]) => params);
  }
}

function make(): { ctx: RunContext; channel: FakeChannel; work: string } {
  works += 1;
  const work = join(folder, `work-${works}`);
  mkdirSync(work);
  const channel = new FakeChannel(join(folder, "store"), work);
  const run = {
    run_id: "r1",
    instance: { id: "a#1", path: "a", step: "a", key: null, take: [1] },
    params: {},
    param_files: {},
    inputs: {},
    work_dir: work,
    resources: {},
    tools: {},
    calls: { "agent.turn": 5 },
    timeout_s: null,
  } as unknown as RunParams;
  return { ctx: new RunContext(run, channel, (text) => text.split(`${folder}/`).join("")), channel, work };
}

const agentOf = (ctx: RunContext, params: Record<string, unknown>) => ctx.agents.get(String(params.agent_id)) as AgentLoop;

const look = tool(
  {
    name: "look",
    description: "Look at something.",
    parameters: {
      type: "object",
      properties: { what: { type: "string" }, n: { type: "integer" } },
      required: ["what"],
    },
  },
  (args) => `saw ${args.what} x${args.n ?? 1}`,
);

// --- tools and their replies ------------------------------------------------------------------

test("a tool's declaration is checked", () => {
  const handler = () => "ok";
  const made = tool({ name: "place_prop", description: "Places a prop.", parameters: {} }, handler);
  expect(made.name).toBe("place_prop");
  expect(made.description).toBe("Places a prop.");
  expect(made.parameters).toEqual({});
  expect(Object.isFrozen(made)).toBe(true);
  tool({ name: "a".repeat(64), description: "d", parameters: { type: "object" } }, handler);
  for (const name of ["Place", "1a", "a".repeat(65), "a-b", "", "a\n", 3]) {
    expect(() => tool({ name: name as string, description: "d", parameters: {} }, handler)).toThrow(
      new TypeError("tool name must be lower_snake_case, at most 64 characters"),
    );
  }
  expect(() => tool({ name: SUBMIT, description: "d", parameters: {} }, handler)).toThrow(
    "submit is reserved for the episode's final answer",
  );
  for (const description of ["", "  \n"]) {
    expect(() => tool({ name: "place", description, parameters: {} }, handler)).toThrow(
      "tool place must carry a description",
    );
  }
  for (const parameters of [[], "object", null, new Map()]) {
    expect(() => tool({ name: "place", description: "d", parameters: parameters as never }, handler)).toThrow(
      "tool place parameters must be a JSON Schema object",
    );
  }
  expect(() =>
    tool({ name: "place", description: "d", parameters: { properties: { x: { default: Number.NaN } } } }, handler),
  ).toThrow("tool place parameters must be a JSON Schema object: at /properties/x/default: NaN is not a JSON number");
  expect(() => tool({ name: "place", description: "d", parameters: {} }, 3 as never)).toThrow(
    "tool place handler must be a function",
  );
  expect(() => tool({ name: "place", description: "d", parameters: {}, handler } as never, handler)).toThrow(
    "a tool declaration has no field 'handler'",
  );
  expect(() => tool(null as never, handler)).toThrow("a tool declaration is an object, not null");
  // The parameters are kept as they were declared.
  const parameters = { type: "object", properties: { x: { type: "number" } } } as const;
  const kept = tool({ name: "keep", description: "d", parameters }, handler);
  (parameters.properties.x as { type: string }).type = "string";
  expect(kept.parameters).toEqual({ type: "object", properties: { x: { type: "number" } } });
});

test("a tool reply holds text and pictures", () => {
  expect(new ToolReply("t").images).toEqual([]);
  expect(new ToolReply("t", ["a.png"]).images).toEqual(["a.png"]);
  expect(() => new ToolReply("t", "a.png" as never)).toThrow("a tool reply's images are a list of pictures, not a string");
});

test("an agent's tools must be declared with tool()", () => {
  const { ctx } = make();
  function plain(): string {
    return "";
  }
  expect(() => ctx.agent({ system: "s", tools: [plain as never] })).toThrow(new NodeFailure("plain is not declared with tool()"));
  expect(() => ctx.agent({ system: "s", tools: [{ name: "x" } as never] })).toThrow("an object is not declared with tool()");
  // A tool named submit from a copy that did not check it.
  const forged = { [Symbol.for("grida.fx.tool")]: { name: "submit", description: "d", parameters: {}, handler: () => null } };
  expect(() => ctx.agent({ system: "s", tools: [forged as never] })).toThrow(
    new NodeFailure("submit is the agent's own tool; name yours otherwise"),
  );
  expect(() => ctx.agent({ system: 3 as never })).toThrow("an agent's system prompt is a string, not a number");
  expect(() => ctx.agent({ system: "s", tools: look as never })).toThrow(
    "an agent's tools are a list of tool() declarations, not an object",
  );
  expect(() => ctx.agent({ system: "s", maxSteps: 3 } as never)).toThrow(
    "ctx.agent takes system, tools, recentImages and maxTokens, not 'maxSteps'",
  );
  expect(ctx.agent({ system: "s", tools: [look] }).transcript).toEqual([]);
});

// --- agent.run --------------------------------------------------------------------------------

test("run sends agent.run", async () => {
  const { ctx, channel } = make();
  const declared = tool(
    { name: "place_prop", description: "Places a prop.", parameters: { type: "object", properties: { x: { type: "number" } } } },
    () => "placed",
  );
  const ref = (await channel.request("file.put", { base64: base64("in"), kind: "image/png", name: "in.png" })) as never;
  const inputFile = new InputFile(ref);
  const loose = join(folder, "loose.JPG");
  writeFileSync(loose, "jpeg");
  const worked = ctx.workPath("views/front.webp");
  writeFileSync(worked, "webp");
  const viaUrl = join(folder, "url.png");
  writeFileSync(viaUrl, "by url");
  channel.requests.length = 0;

  const seen: Record<string, unknown>[] = [];
  const agent = ctx.agent({ system: "sys", tools: [look, declared], recentImages: 1, maxTokens: 50 });
  channel.agentRun = async (params) => {
    seen.push(params);
    expect(agentOf(ctx, params)).toBe(agent as AgentLoop); // registered while pending
    return { submitted: { answer: "good" }, transcript: [{ role: "user", content: "do it" }], turns: 2, cost_usd: 0 };
  };
  const submitted = await agent.run("do it", {
    maxSteps: 5,
    images: [inputFile, PNG_URL, loose, worked, ctx.out.bytes(Buffer.from("o"), "image/gif"), pathToFileURL(viaUrl)],
    submit: { type: "object", properties: { answer: { type: "string" } } },
    check: () => undefined,
  });
  expect(submitted).toEqual({ answer: "good" });
  expect(agent.transcript).toEqual([{ role: "user", content: "do it" }]);
  expect(ctx.agents.size).toBe(0);
  expect(channel.puts()).toEqual([
    { base64: base64("png bytes"), kind: "image/png", name: "agent/image" },
    { base64: base64("jpeg"), kind: "image/jpeg", name: "loose.JPG" },
    { work_path: "views/front.webp", kind: "image/webp", name: "front.webp" },
    { base64: base64("o"), kind: "image/gif", name: "agent/image" },
    { base64: base64("by url"), kind: "image/png", name: "url.png" },
  ]);
  const [params] = channel.requests.filter(([method]) => method === "agent.run").map(([, sent]) => sent);
  expect(params).toEqual({
    agent_id: "agent-1",
    system: "sys",
    instructions: "do it",
    images: [
      { file: (ref as { digest: string }).digest },
      { file: sha256("png bytes") },
      { file: sha256("jpeg") },
      { file: sha256("webp") },
      { file: sha256("o") },
      { file: sha256("by url") },
    ],
    tools: [
      {
        name: "look",
        description: "Look at something.",
        parameters: { type: "object", properties: { what: { type: "string" }, n: { type: "integer" } }, required: ["what"] },
      },
      { name: "place_prop", description: "Places a prop.", parameters: { type: "object", properties: { x: { type: "number" } } } },
    ],
    max_steps: 5,
    submit: { type: "object", properties: { answer: { type: "string" } } },
    check: true,
    recent_images: 1,
    max_tokens: 50,
  });
  // The members in the protocol's order.
  expect(Object.keys(params as object)).toEqual([
    "agent_id",
    "system",
    "instructions",
    "images",
    "tools",
    "max_steps",
    "submit",
    "check",
    "recent_images",
    "max_tokens",
  ]);

  // A text answer, a second agent id, no submit: no check; optional members left out.
  channel.agentRun = async () => ({ text: "done", transcript: [], turns: 1, cost_usd: 0.5 });
  const plain = ctx.agent({ system: "s" });
  expect(await plain.run("go", { maxSteps: 1, check: (() => undefined) as never })).toBe("done");
  const last = channel.requests.at(-1)?.[1] as Record<string, unknown>;
  expect(last.agent_id).toBe("agent-2");
  expect([last.check, last.submit, last.tools]).toEqual([false, null, []]);
  expect(["images", "recent_images", "max_tokens"].filter((key) => key in last)).toEqual([]);
  for (const steps of [0, -1, true, 1.5, "2"]) {
    await expect(plain.run("go", { maxSteps: steps as never })).rejects.toThrow(
      `maxSteps is a whole number of at least 1, not ${typeof steps === "string" ? `'${steps}'` : steps}`,
    );
  }
  await expect(plain.run(3 as never, { maxSteps: 1 })).rejects.toThrow("an agent's instructions are a string, not a number");
  await expect(plain.run("go", { maxSteps: 1, steps: 2 } as never)).rejects.toThrow(
    "agent.run takes maxSteps, images, submit and check, not 'steps'",
  );
  await expect(plain.run("go", { maxSteps: 1, images: "a.png" as never })).rejects.toThrow(
    "images is a list of pictures, not a string",
  );
  await expect(plain.run("go", { maxSteps: 1, submit: "string" as never })).rejects.toThrow(
    "submit is a JSON Schema object, not a string",
  );
  await expect(plain.run("go", { maxSteps: 1, submit: {}, check: 3 as never })).rejects.toThrow("check is a function, not a number");
  // What the engine could not read is never sent.
  await expect(plain.run("go", { maxSteps: 1, submit: { const: Number.NaN } })).rejects.toThrow(
    "agent.run: at /submit/const: NaN is not a JSON number",
  );
});

test("engine errors reach the body", async () => {
  const { ctx, channel } = make();
  channel.agentRun = async () => {
    throw engineError(-32020, "the agent did not finish within 2 turns");
  };
  const agent = ctx.agent({ system: "s", tools: [look] });
  const failed = agent.run("go", { maxSteps: 2 });
  await expect(failed).rejects.toBeInstanceOf(AgentUnfinished);
  await expect(failed).rejects.toThrow("the agent did not finish within 2 turns");
  expect(ctx.agents.size).toBe(0);
  expect(agent.transcript).toEqual([]);
  // An answer that is not an object.
  channel.agentRun = async () => [1];
  await expect(agent.run("go", { maxSteps: 1 })).rejects.toThrow(new EngineError("the engine answered agent.run with an array"));
});

const TRIED = [
  { role: "user", content: "go" },
  { role: "assistant", content: "", tool_calls: [{ id: "c1", name: "look", arguments: '{"what":"x"}' }] },
  { role: "tool", name: "look", tool_call_id: "c1", content: "saw x x1" },
];

test("a failed run keeps the transcript it got to", async () => {
  const { ctx, channel } = make();
  channel.agentRun = async () => {
    throw engineError(-32020, "the agent did not finish within 1 turns", { transcript: TRIED });
  };
  const agent = ctx.agent({ system: "s", tools: [look] });
  let caught: unknown;
  try {
    await agent.run("go", { maxSteps: 1 });
  } catch (error) {
    caught = error;
  }
  expect(caught).toBeInstanceOf(AgentUnfinished);
  expect((caught as AgentUnfinished).data).toEqual({ transcript: TRIED });
  // A body that catches the failure reads what the agent tried.
  expect(agent.transcript).toEqual(TRIED);
  // An error without a transcript (refused before the first turn) leaves none.
  channel.agentRun = async () => {
    throw engineError(-32602, "an agent takes at least 1 step (max_steps)");
  };
  await expect(agent.run("go", { maxSteps: 1 })).rejects.toBeInstanceOf(EngineError);
  expect(agent.transcript).toEqual([]);
});

// --- tool.invoke ------------------------------------------------------------------------------

test("tools are served: every reply form, and what they throw", async () => {
  const { ctx, channel } = make();
  const calls: unknown[] = [];
  const declare = <T>(name: string, handler: (args: Record<string, unknown>) => T): Tool =>
    tool({ name, description: `${name}.`, parameters: { type: "object" } }, handler as never);
  const tools = [
    tool(
      { name: "sync_tool", description: "d", parameters: { type: "object", properties: { what: { type: "string" } } } },
      (args, inner) => {
        calls.push(["sync", args.what, inner.state.mesh, inner === (ctx as never)]);
        return { saw: args.what ?? null, f: 1.5 };
      },
    ),
    declare("async_tool", async () => new ToolReply("here", [PNG_URL])),
    declare("boom", (args) => {
      throw new RangeError(`bad ${String(args.what)} in ${folder}/nodes/a.ts`);
    }),
    declare("says_nothing", () => {
      throw new Error();
    }),
    declare("throws_value", () => {
      throw 42;
    }),
    declare("odd", () => new URL("file:///x.txt")),
    declare("nan", () => Number.NaN),
    declare("nothing", () => undefined),
    declare("null_tool", () => null),
    declare("bad_picture", () => new ToolReply("x", [3 as never])),
    declare("url_picture", () => new ToolReply("x", ["https://example.com/a.png"])),
    declare("http_url", () => new ToolReply("x", [new URL("http://example.com/a.png")])),
    declare("jpeg", () => new ToolReply("placed", [`data:IMAGE/JPEG;base64,${base64("jp")}`])),
    declare("fails", () => {
      throw ctx.fail("the mesh is broken");
    }),
  ];
  ctx.state.mesh = "m.glb";
  const agent = ctx.agent({ system: "s", tools }) as AgentLoop;
  const serve = (name: string, args: Record<string, unknown> = {}) => agent.serveTool(name, args);

  expect(await serve("sync_tool", { what: "x" })).toEqual({ content: { saw: "x", f: 1.5 } });
  expect(await serve("async_tool")).toEqual({ content: "here", images: [{ file: sha256("png bytes") }] });
  expect(await serve("boom", { what: "look" })).toEqual({ error: "RangeError: bad look in nodes/a.ts" });
  expect(await serve("says_nothing")).toEqual({ error: "Error" });
  expect(await serve("throws_value")).toEqual({ error: "NonError: 42" });
  expect(await serve("odd")).toEqual({ content: "file:///x.txt" });
  expect(await serve("nan")).toEqual({ content: "NaN" });
  expect(await serve("nothing")).toEqual({ content: null });
  expect(await serve("null_tool")).toEqual({ content: null });
  expect(await serve("missing")).toEqual({ content: "no tool named missing" });
  expect(await serve("jpeg")).toEqual({ content: "placed", images: [{ file: sha256("jp") }] });
  expect(channel.puts().at(-1)).toEqual({ base64: base64("jp"), kind: "image/jpeg", name: "agent/image" });
  expect(calls).toEqual([["sync", "x", "m.glb", true]]);
  await expect(serve("bad_picture")).rejects.toThrow(new NodeFailure("an agent picture is a file, not a number"));
  await expect(serve("url_picture")).rejects.toThrow(new NodeFailure("an agent picture is a file, not a URL"));
  await expect(serve("http_url")).rejects.toThrow(new NodeFailure("an agent picture is a file, not a URL"));
  await expect(serve("fails")).rejects.toThrow(new NodeFailure("the mesh is broken"));
});

test("data URLs are decoded and stored", async () => {
  const { ctx, channel } = make();
  const agent = ctx.agent({ system: "s", tools: [tool({ name: "show", description: "d", parameters: {} }, (args) => new ToolReply("x", [args.url as string]))] }) as AgentLoop;
  for (const [url, data, kind] of [
    [PNG_URL, "png bytes", "image/png"],
    [`data:IMAGE/WEBP;base64,${base64("w")}`, "w", "image/webp"],
    [`data:;base64,${base64("none")}`, "none", "image/png"],
    // Line breaks inside the payload are dropped: the bytes go out encoded again.
    ["data:image/png;base64,cGlj\ndHVyZQ==", "picture", "image/png"],
  ] as const) {
    expect(await agent.serveTool("show", { url })).toEqual({ content: "x", images: [{ file: sha256(data) }] });
    expect(channel.puts().at(-1)).toEqual({ base64: base64(data), kind, name: "agent/image" });
  }
  // A file value passes as it is; any other object is no picture.
  const passes = tool({ name: "passes", description: "d", parameters: {} }, (args) => new ToolReply("x", [args.value as never]));
  const other = ctx.agent({ system: "s", tools: [passes] }) as AgentLoop;
  expect(await other.serveTool("passes", { value: { file: "a".repeat(64) } })).toEqual({ content: "x", images: [{ file: "a".repeat(64) }] });
  await expect(other.serveTool("passes", { value: { file: "a.png" } })).rejects.toThrow("an agent picture is a file, not an object");
});

test("a tool's failure ends the loop with the tool's own error", async () => {
  const { ctx, channel } = make();
  const failure = ctx.fail("the mesh is broken");
  const fails = tool({ name: "fails", description: "d", parameters: {} }, () => {
    throw failure;
  });
  channel.agentRun = async (params) => {
    try {
      await agentOf(ctx, params).serveTool("fails", {});
    } catch (error) {
      // The engine answers agent.run with the tool's node_failure, and the transcript.
      throw engineError(-32000, (error as Error).message, { transcript: TRIED });
    }
    throw new Error("the tool did not fail");
  };
  const agent = ctx.agent({ system: "s", tools: [fails] });
  let caught: unknown;
  try {
    await agent.run("go", { maxSteps: 3 });
  } catch (error) {
    caught = error;
  }
  expect(caught).toBe(failure);
  expect(caught).not.toBeInstanceOf(EngineError);
  expect(agent.transcript).toEqual(TRIED);
});

// --- agent.check ------------------------------------------------------------------------------

test("the check refuses with a string or a NodeFailure; anything else ends the loop", async () => {
  const { ctx, channel } = make();
  const checked: unknown[] = [];
  const unexpected = new RangeError("boom");
  const check = (value: unknown) => {
    checked.push(value);
    if (value === "short") {
      return "not good enough";
    }
    if (value === "broken") {
      throw ctx.fail("the answer breaks the node");
    }
    if (value === "boom") {
      throw unexpected;
    }
    return value === "nothing" ? null : undefined;
  };
  const results: Record<string, unknown> = {};
  channel.agentRun = async (params) => {
    const agent = agentOf(ctx, params);
    expect(agent.hasCheck).toBe(true);
    for (const value of ["short", "broken", "fine", "nothing"]) {
      results[value] = await agent.serveCheck(value);
    }
    try {
      await agent.serveCheck("boom");
    } catch {
      throw engineError(-32001, "boom");
    }
    throw new Error("the check did not throw");
  };
  const agent = ctx.agent({ system: "s" }) as AgentLoop;
  let caught: unknown;
  try {
    await agent.run("go", { maxSteps: 3, submit: { type: "string" }, check });
  } catch (error) {
    caught = error;
  }
  expect(caught).toBe(unexpected);
  expect(results).toEqual({
    short: { refusal: "not good enough" },
    broken: { refusal: "the answer breaks the node" },
    fine: { refusal: null },
    nothing: { refusal: null },
  });
  expect(checked).toEqual(["short", "broken", "fine", "nothing", "boom"]);
  expect(agent.hasCheck).toBe(false); // only while its run is pending
  expect(await agent.serveCheck("late")).toEqual({ refusal: null });
});

test("an async check, and a check that returns neither a string nor nothing", async () => {
  const { ctx, channel } = make();
  channel.agentRun = async (params) => {
    const agent = agentOf(ctx, params);
    expect(await agent.serveCheck({ a: 1 })).toEqual({ refusal: "try again" });
    return { submitted: { a: 2 }, transcript: [], turns: 2, cost_usd: 0 };
  };
  const answer = await ctx.agent({ system: "s" }).run("go", {
    maxSteps: 2,
    submit: { type: "object" },
    check: async () => {
      await new Promise((done) => setTimeout(done, 1));
      return "try again";
    },
  });
  expect(answer).toEqual({ a: 2 });

  channel.agentRun = async (params) => {
    try {
      await agentOf(ctx, params).serveCheck(1);
    } catch (error) {
      expect(error).toBeInstanceOf(TypeError);
      throw engineError(-32001, (error as Error).message);
    }
    throw new Error("the check was accepted");
  };
  await expect(
    ctx.agent({ system: "s" }).run("go", { maxSteps: 1, submit: { type: "integer" }, check: (() => false) as never }),
  ).rejects.toThrow(new TypeError("a check returns a refusal (a string) or nothing, not a boolean"));
});
