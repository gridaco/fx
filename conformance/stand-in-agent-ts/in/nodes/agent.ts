// A node type of the stand-in-agent-ts case: an agent with one tool that must submit a height,
// stand-in-agent's nodes/agent.py in TypeScript. The tool's description and parameters are the
// ones Python derives from `measure`'s docstring and signature, so its calls have the same keys.

import { node, tool } from "@grida/fx";

const HEIGHT = {
  type: "object",
  properties: { height: { type: "integer" }, unit: { type: "string" } },
  required: ["height", "unit"],
  additionalProperties: false,
} as const;

const measure = tool(
  {
    name: "measure",
    description: "Measures a thing and says how tall it is.",
    parameters: { type: "object", properties: { thing: { type: "string" } }, required: ["thing"] },
  },
  (args, ctx) => {
    const measured = (ctx.state.measured ??= []) as string[];
    measured.push(args.thing);
    return { thing: args.thing, height: 3, unit: "hands" };
  },
);

export const survey = node(
  { name: "survey", params: { task: "string" }, outputs: { answer: "json" }, calls: { "agent.turn": 3 }, version: 1 },
  async (ctx) => {
    const agent = ctx.agent({ system: "You measure things before you answer.", tools: [measure] });
    const submitted = await agent.run(ctx.params.task, { maxSteps: 3, submit: HEIGHT });
    await ctx.fact("measured", (ctx.state.measured ?? []) as string[]);
    await ctx.fact(
      "transcript_roles",
      agent.transcript.map((message) => (message as { role: string }).role),
    );
    return { answer: ctx.out.json({ submitted, transcript: [...agent.transcript] }) };
  },
);
