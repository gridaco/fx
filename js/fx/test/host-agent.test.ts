// The TypeScript node host's agents (src/host.ts `serveAgents`, src/ctx.ts `AgentLoop`): a body's
// `agent.run`, with the engine's `tool.invoke` and `agent.check` served while it is pending,
// `$/cancel` of a slow tool, and failures that end the loop. The cases follow
// python/tests/test_host_run.py's agent tests.

import { afterEach, describe, expect, test } from "bun:test";
import {
  expectValid,
  FakeEngine,
  HOST_SKIP,
  HOST_TEST,
  type HostProcess,
  type Message,
  makeProject,
  runParams,
  sha256,
  start,
  stopAll,
} from "./host-support.js";

afterEach(stopAll);

const AGENTS = `import { writeFileSync } from "node:fs";
import { node, tool, ToolReply } from "@grida/fx";

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
  (args, ctx) => {
    const looked = (ctx.state.looked ??= []) as string[];
    looked.push(args.what);
    if (args.what === "boom") {
      throw new Error("bad look");
    }
    return { saw: args.what, n: args.n ?? 1, mesh: ctx.state.mesh as string };
  },
);

const picture = tool({ name: "picture", description: "Shows the front.", parameters: { type: "object" } }, (_args, ctx) => {
  const path = ctx.workPath("views/front.png");
  writeFileSync(path, "front");
  return new ToolReply("front view", [path]);
});

const slow = tool({ name: "slow", description: "Takes its time.", parameters: { type: "object" } }, async (_args, ctx) => {
  await new Promise((done) => setTimeout(done, 400));
  ctx.state.slowDone = true;
  return "late";
});

const breaks = tool({ name: "breaks", description: "Breaks the node.", parameters: { type: "object" } }, (_args, ctx) => {
  throw ctx.fail("the room is broken");
});

class OutsideTheRoom extends Error {
  constructor(message: string) {
    super(message);
    this.name = "OutsideTheRoom";
  }
}

const place = tool(
  { name: "place", description: "Places a prop.", parameters: { type: "object", properties: { x: { type: "number" } } } },
  (args) => {
    if ((args.x ?? 0) > 1) {
      throw new OutsideTheRoom("x is outside the room");
    }
    return new ToolReply("placed", ["data:image/png;base64," + Buffer.from("placed").toString("base64")]);
  },
);

export const agent = node({ name: "agent", outputs: { answer: "json" }, calls: { "agent.turn": 5 } }, async (ctx) => {
  ctx.state.mesh = "m.glb";
  const runner = ctx.agent({ system: "You place props.", tools: [look, picture, slow, breaks, place], recentImages: 1 });
  const answer = await runner.run("do it", {
    maxSteps: 5,
    submit: { type: "object" },
    check: (value) => (value.answer === "bad" ? "bad is not good enough" : undefined),
  });
  await ctx.fact("transcript_length", runner.transcript.length);
  await ctx.fact("looked", ctx.state.looked as string[]);
  return { answer: ctx.out.json(answer) };
});

export const agentFails = node({ name: "agent_fails", calls: { "agent.turn": 1 } }, async (ctx) => {
  await ctx.fact("before", 1);
  await ctx.agent({ system: "s", tools: [breaks] }).run("go", { maxSteps: 1 });
});

function badCheck(): never {
  throw new RangeError("unexpected");
}

export const agentCheckBreaks = node({ name: "agent_check_breaks", calls: { "agent.turn": 1 } }, async (ctx) => {
  await ctx.agent({ system: "s" }).run("go", { maxSteps: 1, submit: { type: "object" }, check: badCheck });
});

export const twoAgents = node({ name: "two_agents", calls: { "agent.turn": 2 } }, async (ctx) => {
  const first = await ctx.agent({ system: "s" }).run("one", { maxSteps: 1 });
  const second = await ctx.agent({ system: "s", tools: [look] }).run("two", { maxSteps: 1 });
  await ctx.fact("answers", [first, second]);
});
`;

/** A host over a project of agent bodies, initialized, answered by a fake engine. */
async function started(): Promise<{ host: HostProcess; engine: FakeEngine }> {
  const host = start(makeProject({ "nodes/agents.ts": AGENTS }));
  const engine = new FakeEngine();
  host.engine = engine.answering;
  await host.initialize();
  return { host, engine };
}

/** Starts a run of `attribute` and waits for its `agent.run`, serving the host's other requests. */
async function untilAgentRun(host: HostProcess, engine: FakeEngine, attribute: string, runId = "r1") {
  const id = host.ask(
    "run",
    runParams({ path: "nodes/agents.ts", attribute }, { run_id: runId, work_dir: engine.newWorkDir(), calls: { "agent.turn": 5 } }),
  );
  for (;;) {
    const message = await host.next();
    if (message.method === "agent.run") {
      expectValid(message.params, "agent_run_params");
      host.asked.push(message);
      return { id, agentRun: message };
    }
    host.serve(message);
  }
}

const invokeParams = (agentId: unknown, name: string, args: Record<string, unknown>, over: Record<string, unknown> = {}) => ({
  run_id: "r1",
  agent_id: agentId,
  call_id: `c-${name}`,
  name,
  arguments: args,
  ...over,
});

/** A result, held to its schema. */
function result(answer: Message, name: string): unknown {
  if (answer.result === undefined) {
    throw new Error(`no result: ${JSON.stringify(answer)}`);
  }
  expectValid(answer.result, name);
  return answer.result;
}

describe.skipIf(HOST_SKIP !== null)("the TypeScript node host's agents", () => {
  test(
    "tool.invoke and agent.check with no run pending",
    async () => {
      const { host } = await started();
      expect((await host.request("tool.invoke", invokeParams("agent-1", "look", {}))).error).toEqual({
        code: -32602,
        message: "no run r1 is pending on this host",
      });
      expect((await host.request("agent.check", { run_id: "r1", agent_id: "agent-1", value: 1 })).error).toEqual({
        code: -32602,
        message: "no run r1 is pending on this host",
      });
      expect((await host.request("tool.invoke", { agent_id: "agent-1" })).error).toEqual({
        code: -32602,
        message: "no run null is pending on this host",
      });
      expect((await host.request("tool.invoke", [])).error).toEqual({
        code: -32602,
        message: "this method's params are a JSON object",
      });
    },
    HOST_TEST,
  );

  test(
    "an agent run serves its tools and its check, and $/cancel answers a slow tool",
    async () => {
      const { host, engine } = await started();
      const { id, agentRun } = await untilAgentRun(host, engine, "agent");
      const params = agentRun.params as Record<string, unknown>;
      const agentId = params.agent_id;
      expect(agentId).toBe("agent-1");
      const tools = params.tools as Record<string, unknown>[];
      expect(tools.map((item) => item.name)).toEqual(["look", "picture", "slow", "breaks", "place"]);
      expect(tools[0]).toEqual({
        name: "look",
        description: "Look at something.",
        parameters: {
          type: "object",
          properties: { what: { type: "string" }, n: { type: "integer" } },
          required: ["what"],
        },
      });
      expect(tools[4]?.parameters).toEqual({ type: "object", properties: { x: { type: "number" } } });
      expect([params.run_id, params.submit, params.check, params.max_steps, params.recent_images]).toEqual([
        "r1",
        { type: "object" },
        true,
        5,
        1,
      ]);
      expect("max_tokens" in params || "images" in params).toBe(false);

      const invoke = async (name: string, args: Record<string, unknown>, over: Record<string, unknown> = {}) =>
        host.request("tool.invoke", invokeParams(agentId, name, args, over));
      expect(result(await invoke("look", { what: "x" }), "tool_invoke_result")).toEqual({
        content: { saw: "x", n: 1, mesh: "m.glb" },
      });
      expect(result(await invoke("look", { what: "boom" }), "tool_invoke_result")).toEqual({ error: "Error: bad look" });
      expect(result(await invoke("picture", {}), "tool_invoke_result")).toEqual({
        content: "front view",
        images: [{ file: sha256("front") }],
      });
      expect(result(await invoke("place", { x: 2 }), "tool_invoke_result")).toEqual({
        error: "OutsideTheRoom: x is outside the room",
      });
      expect(result(await invoke("place", { x: 0.5 }), "tool_invoke_result")).toEqual({
        content: "placed",
        images: [{ file: sha256("placed") }],
      });
      expect(result(await invoke("nothing", {}), "tool_invoke_result")).toEqual({ content: "no tool named nothing" });
      expect((await invoke("look", { what: "x" }, { agent_id: "agent-9" })).error).toEqual({
        code: -32602,
        message: "run r1 has no agent agent-9 running",
      });
      expect((await invoke("look", { what: "x" }, { run_id: "r9" })).error).toEqual({
        code: -32602,
        message: "no run r9 is pending on this host",
      });
      for (const [over, message] of [
        [{ call_id: 3 }, "tool.invoke's call_id is a string or null"],
        [{ name: "" }, "tool.invoke's name is a tool's name"],
        [{ arguments: [] }, "tool.invoke's arguments are a JSON object"],
      ] as const) {
        expect((await invoke("look", {}, over)).error).toEqual({ code: -32602, message });
      }
      const noCallId = invokeParams(agentId, "look", { what: "y" }) as Record<string, unknown>;
      delete noCallId.call_id;
      expect((await host.request("tool.invoke", noCallId)).error).toEqual({
        code: -32602,
        message: "tool.invoke's call_id is a string or null",
      });
      // A slow tool, cancelled: answered at once; its late answer is never sent.
      const slow = host.ask("tool.invoke", invokeParams(agentId, "slow", {}, { call_id: null }));
      host.notify("$/cancel", { id: slow });
      expect((await host.answerTo(slow)).error).toEqual({ code: -32002, message: "the engine cancelled tool.invoke" });

      const check = { run_id: "r1", agent_id: agentId };
      expect(result(await host.request("agent.check", { ...check, value: { answer: "bad" } }), "agent_check_result")).toEqual({
        refusal: "bad is not good enough",
      });
      expect(result(await host.request("agent.check", { ...check, value: { answer: "good" } }), "agent_check_result")).toEqual({
        refusal: null,
      });
      expect((await host.request("agent.check", check)).error).toEqual({
        code: -32602,
        message: "agent.check needs the submitted value",
      });
      // After shutdown, the pending run's requests are still served.
      expect((await host.request("shutdown")).result).toBeNull();
      expect(result(await invoke("look", { what: "after" }), "tool_invoke_result")).toEqual({
        content: { saw: "after", n: 1, mesh: "m.glb" },
      });
      expect((await host.request("describe", { targets: [], builtins: false })).error).toEqual({
        code: -32600,
        message: "describe came after shutdown",
      });
      await new Promise((done) => setTimeout(done, 500));
      expect(host.pending()).toBe(0);

      const transcript = [
        { role: "user", content: "do it" },
        { role: "assistant", content: "", tool_calls: [] },
        { role: "user", content: "Finish by calling submit." },
      ];
      host.send({
        jsonrpc: "2.0",
        id: agentRun.id,
        result: { submitted: { answer: "good" }, transcript, turns: 3, cost_usd: 0 },
      });
      const answer = await host.answerTo(id);
      expectValid(answer.result, "run_result");
      const outputs = (answer.result as { outputs: Record<string, { file: { kind: string; digest: string } }> }).outputs;
      expect(outputs.answer?.file.kind).toBe("json");
      expect(outputs.answer?.file.digest).toBe(sha256(JSON.stringify({ answer: "good" })));
      const puts = host.asked.filter((message) => message.method === "file.put").map((message) => message.params as Record<string, unknown>);
      expect(puts[0]).toEqual({ run_id: "r1", work_path: "views/front.png", kind: "image/png", name: "front.png" });
      expect(puts[1]?.base64).toBe(Buffer.from("placed").toString("base64"));
      const facts = Object.fromEntries(
        host.asked
          .filter((message) => message.method === "fact")
          .map((message) => [(message.params as { name: string }).name, (message.params as { value: unknown }).value]),
      );
      expect(facts).toEqual({ transcript_length: 3, looked: ["x", "boom", "after"] });
      host.notify("exit");
      expect((await host.ended()).code).toBe(0);
    },
    HOST_TEST,
  );

  test(
    "a tool that fails the node ends the loop with the tool's own failure",
    async () => {
      const { host, engine } = await started();
      const { id, agentRun } = await untilAgentRun(host, engine, "agentFails");
      const agentId = (agentRun.params as { agent_id: string }).agent_id;
      const failed = await host.request("tool.invoke", invokeParams(agentId, "breaks", {}));
      expect(failed.error).toEqual({ code: -32000, message: "the room is broken" });
      host.send({
        jsonrpc: "2.0",
        id: agentRun.id,
        error: { code: -32000, message: "the room is broken", data: { transcript: [] } },
      });
      expect((await host.answerTo(id)).error).toEqual({
        code: -32000,
        message: "the room is broken",
        data: { facts: { before: 1 }, marks: [] },
      });
    },
    HOST_TEST,
  );

  test(
    "a check that throws is a node_error, and so is the run",
    async () => {
      const { host, engine } = await started();
      const { id, agentRun } = await untilAgentRun(host, engine, "agentCheckBreaks");
      const agentId = (agentRun.params as { agent_id: string }).agent_id;
      const checked = await host.request("agent.check", { run_id: "r1", agent_id: agentId, value: {} });
      const error = checked.error as { code: number; message: string; data: { exception: string; traceback: string } };
      expect([error.code, error.message, error.data.exception]).toEqual([-32001, "unexpected", "RangeError"]);
      // The traceback starts at the check, not in the SDK.
      const lines = error.data.traceback.split("\n");
      expect(lines[0]).toBe("RangeError: unexpected");
      expect(lines[1]).toMatch(/^ +at badCheck \(nodes\/agents\.ts:\d+:\d+\)$/);
      expect(error.data.traceback).not.toContain("ctx.js");
      host.send({ jsonrpc: "2.0", id: agentRun.id, error: { code: -32001, message: "unexpected" } });
      const run = (await host.answerTo(id)).error as { code: number; message: string; data: { exception: string; traceback: string } };
      expect([run.code, run.message, run.data.exception]).toEqual([-32001, "unexpected", "RangeError"]);
      expect(run.data.traceback).toContain("at badCheck (nodes/agents.ts:");
    },
    HOST_TEST,
  );

  test(
    "each agent of a run has its id, and only a run with a check is asked",
    async () => {
      const { host, engine } = await started();
      const { id, agentRun } = await untilAgentRun(host, engine, "twoAgents");
      expect((agentRun.params as { agent_id: string }).agent_id).toBe("agent-1");
      expect((await host.request("agent.check", { run_id: "r1", agent_id: "agent-1", value: 1 })).error).toEqual({
        code: -32602,
        message: "agent agent-1 has no check",
      });
      host.send({ jsonrpc: "2.0", id: agentRun.id, result: { text: "first", transcript: [], turns: 1, cost_usd: 0 } });
      const second = await host.next();
      expect(second.method).toBe("agent.run");
      expect((second.params as { agent_id: string }).agent_id).toBe("agent-2");
      // The first agent is done: its id names nothing now.
      expect((await host.request("tool.invoke", invokeParams("agent-1", "look", { what: "x" }))).error).toEqual({
        code: -32602,
        message: "run r1 has no agent agent-1 running",
      });
      host.send({ jsonrpc: "2.0", id: second.id, result: { text: "second", transcript: [], turns: 1, cost_usd: 0 } });
      expect((await host.answerTo(id)).result).toEqual({ outputs: {} });
      expect(host.asked.find((message) => message.method === "fact")?.params).toEqual({
        run_id: "r1",
        name: "answers",
        value: ["first", "second"],
      });
    },
    HOST_TEST,
  );
});
