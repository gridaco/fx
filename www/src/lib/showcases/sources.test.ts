import { expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { workflows } from "../workflows";
import { workflowSource } from "./sources";

test("every showcase points to its complete tracked authoring project", () => {
  const root = resolve(import.meta.dir, "../../../..");
  for (const workflow of workflows) {
    const source = workflowSource(workflow.slug);
    const home = resolve(root, "examples", source.folder);
    const [file, builder] = source.target.split(":");
    expect(existsSync(resolve(home, "README.md"))).toBe(true);
    expect(existsSync(resolve(home, "fx.yaml"))).toBe(true);
    expect(existsSync(resolve(home, file!))).toBe(true);
    if (builder) expect(readFileSync(resolve(home, file!), "utf8")).toContain(`def ${builder}(`);
    if (source.inputs) expect(existsSync(resolve(home, source.inputs))).toBe(true);
    expect(source.command).toContain(`grida fx run ${source.target}`);
    expect(source.command).toContain("grida fx start --background");
    expect(source.url).toEndWith(`/examples/${source.folder}`);
    const ref = workflow.slug === "product-promo" || workflow.slug === "character-performance"
      ? "7e9657df53c93ceba47f82e729d6c3573efeedfb"
      : "main";
    expect(source.url).toBe(`https://github.com/gridaco/fx/tree/${ref}/examples/${source.folder}`);
  }
});
