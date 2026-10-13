// A module and a body that write to standard output every way a TypeScript body can: none of it
// may reach the node protocol, which the TypeScript node host keeps on a descriptor of its own.

import { spawn, spawnSync } from "node:child_process";
import { node } from "@grida/fx";

console.log("marker-module: console.log while the module loads");

export const noisy = node(
  { name: "noisy", params: { word: "string" }, outputs: { text: "text" } },
  async (ctx) => {
    console.log("marker-console: console.log");
    console.info("marker-info: console.info");
    process.stdout.write("marker-write: process.stdout.write\n");
    spawnSync("sh", ["-c", "echo marker-sync: spawnSync with inherited stdio"], { stdio: "inherit" });
    await new Promise<void>((resolve, reject) => {
      const child = spawn("sh", ["-c", "echo marker-async: spawn with inherited stdio"], { stdio: "inherit" });
      child.on("error", reject);
      child.on("exit", () => resolve());
    });
    // `; true` keeps the outer shell from replacing itself with the inner one.
    spawnSync("sh", ["-c", `sh -c "echo marker-grandchild: a child of a child"; true`], { stdio: "inherit" });
    // A child reads nothing of the protocol either: its standard input is empty.
    const read = spawnSync("sh", ["-c", "cat"], { stdio: ["inherit", "pipe", "inherit"] });
    console.log(`marker-stdin: a child read ${read.stdout.length} bytes`);
    return { text: ctx.out.text(`${ctx.params.word}\n`) };
  },
);
