import { expect, test } from "bun:test";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { expr, plan, toYaml, workflow } from "../src/index.js";
import { HOST_SKIP, makeProject } from "./host-support.js";
import { SLOW, testEngine } from "./support.js";

const engine = testEngine();

test.skipIf(engine === null || HOST_SKIP !== null)("SDK node defaults and effective input values exist before a body executes", async () => {
  const cwd = makeProject({
    "fx.yaml": "fx: project/v1\n",
    "nodes.ts": `import { node } from "@grida/fx";
export const echo = node({name:"acme_echo",params:{p1:"string",p2:{type:"string",default:"schema default"}},outputs:{text:"text/plain"}}, () => {throw new Error("body must not run while planning");});`,
  });
  writeFileSync(join(cwd, "case.yaml"), toYaml(workflow({
    id: "case", title: "Default value inspection",
    inputs: { prompt: { type: "string", default: "A lantern" } },
    steps: { echo: { uses: "./nodes.ts#echo", with: { p1: expr("inputs.prompt") } } },
    outputs: { message: expr("steps.echo.outputs.text") },
  })));
  const result = await plan("case.yaml", { cwd, env: { GRIDA_FX_BIN: engine!, GRIDA_FX_NETWORK: "off", GRIDA_FX_DISABLE_DOTENV: "1" } });
  expect(result.ok).toBe(true);
  expect(result.graph.instances[0]!.with.p2).toBe("schema default");
  const values = result.graph.value_views as Record<string, unknown>[];
  expect(values.some(value => value.kind === "input" && value.name === "prompt" && JSON.stringify(value.data) === JSON.stringify({ value: "A lantern" }))).toBe(true);
  expect(values.some(value => value.kind === "output" && value.state === "pending")).toBe(true);
}, SLOW);
