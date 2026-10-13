// A workflow builder: `grida-fx run workflows/build.ts:build --arg count=3`. Its one argument is
// an object of the --arg strings; it returns the workflow document that workflow() makes.

import { expr, type Step, workflow } from "@grida/fx";

export function build(args: { readonly [name: string]: string }) {
  const count = Number(args.count ?? "2");
  const steps: { [name: string]: Step } = {};
  const outputs: { [name: string]: string } = {};
  for (let number = 1; number <= count; number++) {
    steps[`line_${number}`] = { uses: "./nodes/make.ts#make", with: { text: `line ${number} of ${count}` } };
    outputs[`line_${number}`] = expr(`steps.line_${number}.outputs.text`);
  }
  return workflow({ id: "built", title: `${count} lines from a TypeScript builder`, steps, outputs });
}

// Not a builder: it returns the steps, not a workflow.
export function steps() {
  return [{ uses: "./nodes/make.ts#make" }];
}
