// Two bodies that run past their step's timeout. Each counts its runs in a file in the project
// folder, the node host's working directory, before it waits.

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { node } from "@grida/fx";

function count(file: string): void {
  const runs = existsSync(file) ? Number(readFileSync(file, "utf8")) + 1 : 1;
  writeFileSync(file, String(runs));
}

// Sleeps without looking at ctx.signal: the engine ends its host.
export const slow = node(
  { name: "slow", params: { seconds: "number" }, outputs: { text: "text" }, version: 1, retry: "engine" },
  async (ctx) => {
    count("naps.txt");
    await new Promise((resolve) => setTimeout(resolve, ctx.params.seconds * 1000));
    return { text: ctx.out.text("awake") };
  },
);

// Waits for ctx.signal, which `$/cancel` aborts, writes down why, and gives up.
export const watchful = node(
  { name: "watchful", params: { seconds: "number" }, outputs: { text: "text" }, version: 1, retry: "engine" },
  async (ctx) => {
    count("watches.txt");
    const signal = ctx.signal;
    await new Promise<void>((resolve) => {
      const timer = setTimeout(resolve, ctx.params.seconds * 1000);
      signal.addEventListener("abort", () => {
        clearTimeout(timer);
        resolve();
      });
    });
    if (signal.aborted) {
      const reason = signal.reason instanceof Error ? signal.reason.message : String(signal.reason);
      writeFileSync("aborted.txt", `${reason}\n`);
      throw ctx.fail("stopped waiting");
    }
    return { text: ctx.out.text("awake") };
  },
);
