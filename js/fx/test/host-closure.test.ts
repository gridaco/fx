// Source closures of TypeScript modules (src/closure.ts), as `describe` reports them: transitive
// imports, `#` subpath imports, type-only imports, dependencies under node_modules, JSON and
// `require` (cached by Node or not), declared source packages and symlinked workspace packages,
// closures that do not depend on what the session loaded before (shared registries, CommonJS
// helpers, top-level await), the evaluation marker (no name a module can shadow, columns and
// headers as Node reads them), the run-time guard against what other modules loaded, exact
// spelling (case and Unicode normalization), CommonJS refusals whatever loaded the module first,
// and `@grida/fx` resolved from the host when the project has none (for `import` and `require`);
// then literal specifiers, which count whether or not they run (memoized lazy loads, code that
// never runs, and what is not code), `createRequire` from a parent Node never loaded, runs scoped
// by their async context, a user `--conditions=require`, the stack read leaving `Error` as it was,
// and a CommonJS module that left `require.cache`.

import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  builtHost,
  FakeEngine,
  HOST_SKIP,
  HOST_TEST,
  type HostProcess,
  makeProject,
  runParams,
  start,
  stopAll,
} from "./host-support.js";

afterEach(stopAll);

const NODE = (imports: string, name = "uses") =>
  `import { node } from "@grida/fx";\n${imports}\nexport const ${name} = node({ name: "${name}" }, () => ({}));\n`;

type Entry = { label: string; path: string };

async function closureOf(root: string, path: string, sources: string[] = []): Promise<Record<string, unknown>> {
  const host = start(root);
  await host.initialize(sources);
  const { modules } = await host.describe([{ path }]);
  if (modules[0]?.error !== undefined) {
    throw new Error(String(modules[0].error));
  }
  return modules[0] as Record<string, unknown>;
}

const labels = (module: Record<string, unknown>) => (module.closure as Entry[]).map((entry) => entry.label);

/** Describes `order` in a new host and answers each module's closure labels by path. */
async function closuresIn(root: string, order: readonly string[]): Promise<Record<string, string[]>> {
  const host = start(root);
  await host.initialize();
  const { modules } = await host.describe(order.map((path) => ({ path })));
  return Object.fromEntries(
    modules.map((module) => {
      if (module.error !== undefined) {
        throw new Error(String(module.error));
      }
      return [String(module.path), labels(module)];
    }),
  );
}

/** Runs `attribute` of the module at `path` and answers its result, or its error message. */
async function runOf(host: HostProcess, path: string, attribute: string, runId: string): Promise<unknown> {
  const engine = new FakeEngine();
  host.engine = engine.answering;
  const params = runParams({ path, attribute }, { run_id: runId, work_dir: engine.newWorkDir() });
  const answer = await host.request("run", params);
  return answer.error !== undefined ? answer.error.message : answer.result;
}

const late = (label: string) => `the body loaded ${label} at run time, outside its source closure: import it statically`;

/** Whether this file system ignores case where the project lives. */
function ignoresCase(root: string): boolean {
  return existsSync(join(root, "PACKAGE.JSON"));
}

describe.skipIf(HOST_SKIP !== null)("source closures", () => {
  test(
    "transitive imports, # subpath imports, JSON and require count; a type-only import does not",
    async () => {
      const root = makeProject({
        "package.json": JSON.stringify({ type: "module", imports: { "#lib/*": "./lib/*" } }),
        "nodes/a.ts": NODE(
          [
            'import { b } from "../lib/b.ts";',
            'import { png } from "#lib/png.ts";',
            'import type { Shape } from "../lib/types.ts";',
            'import data from "../data/table.json" with { type: "json" };',
            'import { createRequire } from "node:module";',
            'const required = createRequire(import.meta.url)("../data/required.json");',
            "export const all = [b, png, data, required];",
            'export const shape: Shape = "round";',
          ].join("\n"),
        ),
        "lib/b.ts": 'import { c } from "./c.ts";\nexport const b = c + 1;\n',
        "lib/c.ts": "export const c = 1;\n",
        "lib/png.ts": "export const png = 2;\n",
        "lib/types.ts": 'export type Shape = "round";\n',
        "lib/unused.ts": "export const unused = 3;\n",
        "data/table.json": '{"a": 1}\n',
        "data/required.json": "[1]\n",
      });
      const module = await closureOf(root, "nodes/a.ts");
      expect(labels(module)).toEqual([
        "data/required.json",
        "data/table.json",
        "lib/b.ts",
        "lib/c.ts",
        "lib/png.ts",
        "nodes/a.ts",
      ]);
      for (const entry of module.closure as Entry[]) {
        expect(entry.path).toBe(join(root, ...entry.label.split("/")));
      }
    },
    HOST_TEST,
  );

  test(
    "a helper another module loaded first is still in each closure",
    async () => {
      const root = makeProject({
        "nodes/first.ts": NODE('import { shared } from "../lib/shared.ts";\nexport const s = shared;', "first"),
        "nodes/second.ts": NODE('import { shared } from "../lib/shared.ts";\nexport const s = shared;', "second"),
        "lib/shared.ts": 'import { deep } from "./deep.ts";\nexport const shared = deep;\n',
        "lib/deep.ts": "export const deep = 1;\n",
      });
      const host = start(root);
      await host.initialize();
      const { modules } = await host.describe([{ path: "nodes/first.ts" }, { path: "nodes/second.ts" }]);
      expect(labels(modules[0] as Record<string, unknown>)).toEqual(["lib/deep.ts", "lib/shared.ts", "nodes/first.ts"]);
      expect(labels(modules[1] as Record<string, unknown>)).toEqual(["lib/deep.ts", "lib/shared.ts", "nodes/second.ts"]);
    },
    HOST_TEST,
  );

  test(
    "dependencies under node_modules are not source, and what they import is not followed",
    async () => {
      const root = makeProject({
        "nodes/a.ts": NODE('import { dep } from "dep";\nimport { cjs } from "old-dep";\nexport const d = [dep, cjs];'),
        "node_modules/dep/package.json": JSON.stringify({ name: "dep", type: "module", exports: "./index.js" }),
        "node_modules/dep/index.js": 'import { inner } from "./inner.js";\nexport const dep = inner;\n',
        "node_modules/dep/inner.js": "export const inner = 1;\n",
        // A dependency may be CommonJS.
        "node_modules/old-dep/package.json": JSON.stringify({ name: "old-dep", main: "index.js" }),
        "node_modules/old-dep/index.js": "exports.cjs = 2;\n",
      });
      expect(labels(await closureOf(root, "nodes/a.ts"))).toEqual(["nodes/a.ts"]);
    },
    HOST_TEST,
  );

  test(
    "a project file only a dependency loads is in no closure, whenever the dependency loads it",
    async () => {
      const root = makeProject({
        "node_modules/loader/package.json": JSON.stringify({ name: "loader", main: "index.js" }),
        "node_modules/loader/index.js":
          'exports.load = (url) => import(url);\nexports.need = (path) => require(path);\n',
        "node_modules/esm-loader/package.json": JSON.stringify({ name: "esm-loader", type: "module", exports: "./index.js" }),
        "node_modules/esm-loader/index.js": "export const load = (url) => import(url);\n",
        "config/a.ts": "export const a = 1;\n",
        "config/b.ts": "export const b = 2;\n",
        "config/c.json": "3\n",
        "nodes/a.ts": NODE(
          [
            'import loader from "loader";',
            'import { load } from "esm-loader";',
            'const here = (path: string) => new URL(path, import.meta.url);',
            'export const a = (await loader.load(here("../config/a.ts").href)).a;',
            'export const b = (await load(here("../config/b.ts").href)).b;',
            'export const c = loader.need(here("../config/c.json").pathname);',
          ].join("\n"),
        ),
      });
      expect(labels(await closureOf(root, "nodes/a.ts"))).toEqual(["nodes/a.ts"]);
    },
    HOST_TEST,
  );

  test(
    "a declared source package counts by its name; an undeclared workspace package by its real path",
    async () => {
      const root = makeProject({
        "nodes/a.ts": NODE(
          'import { shape } from "@acme/shapes";\nimport { color } from "@acme/colors";\nimport { tint } from "tints";\nexport const x = [shape, color, tint];',
        ),
        // A workspace package, linked into node_modules.
        "packages/shapes/package.json": JSON.stringify({ name: "@acme/shapes", type: "module", exports: "./src/index.ts" }),
        "packages/shapes/src/index.ts": 'import { edge } from "./edge.ts";\nexport const shape: string = "round" + edge;\n',
        "packages/shapes/src/edge.ts": 'export const edge = "!";\n',
        "packages/tints/package.json": JSON.stringify({ name: "tints", type: "module", exports: "./index.ts" }),
        "packages/tints/index.ts": 'export const tint: string = "light";\n',
        // An installed package, JavaScript.
        "node_modules/@acme/colors/package.json": JSON.stringify({ name: "@acme/colors", type: "module", exports: "./index.js" }),
        "node_modules/@acme/colors/index.js": 'import { red } from "./red.js";\nexport const color = red;\n',
        "node_modules/@acme/colors/red.js": 'export const red = "red";\n',
      });
      mkdirSync(join(root, "node_modules", "@acme"), { recursive: true });
      symlinkSync(join(root, "packages", "shapes"), join(root, "node_modules", "@acme", "shapes"));
      symlinkSync(join(root, "packages", "tints"), join(root, "node_modules", "tints"));
      const module = await closureOf(root, "nodes/a.ts", ["@acme/shapes", "@acme/colors", "@acme/absent"]);
      expect(module.closure).toEqual([
        { label: "@acme/colors/index.js", path: join(root, "node_modules", "@acme", "colors", "index.js") },
        { label: "@acme/colors/red.js", path: join(root, "node_modules", "@acme", "colors", "red.js") },
        { label: "@acme/shapes/src/edge.ts", path: join(root, "packages", "shapes", "src", "edge.ts") },
        { label: "@acme/shapes/src/index.ts", path: join(root, "packages", "shapes", "src", "index.ts") },
        { label: "nodes/a.ts", path: join(root, "nodes", "a.ts") },
        { label: "packages/tints/index.ts", path: join(root, "packages", "tints", "index.ts") },
      ]);
      // Undeclared, the installed package is a dependency like any other.
      expect(labels(await closureOf(root, "nodes/a.ts"))).toEqual([
        "nodes/a.ts",
        "packages/shapes/src/edge.ts",
        "packages/shapes/src/index.ts",
        "packages/tints/index.ts",
      ]);
    },
    HOST_TEST,
  );

  test(
    "a file counts only when it is spelled as its directory entries spell it",
    async () => {
      const root = makeProject({
        "nodes/wrongcase.ts": NODE('import { png } from "../lib/PNG.ts";\nexport const p = png;'),
        "nodes/right.ts": NODE('import { png } from "../lib/png.ts";\nexport const p = png;', "right"),
        "lib/png.ts": "export const png = 1;\n",
      });
      const host = start(root);
      await host.initialize();
      const { modules } = await host.describe([
        { path: "nodes/wrongcase.ts" },
        { path: "nodes/right.ts" },
        { path: "Nodes/right.ts" },
      ]);
      const [wrong, right, wrongTarget] = modules;
      expect(labels(right as Record<string, unknown>)).toEqual(["lib/png.ts", "nodes/right.ts"]);
      if (ignoresCase(root)) {
        expect(wrong?.error).toBe("nodes/wrongcase.ts failed to import: lib/PNG.ts is spelled lib/png.ts on disk");
        expect(wrongTarget?.error).toBe("Nodes/right.ts failed to import: Nodes/right.ts is spelled nodes/right.ts on disk");
      } else {
        // Node itself refuses the name on a file system that keeps case apart.
        expect(wrong?.error).toBe(
          "nodes/wrongcase.ts failed to import: ERR_MODULE_NOT_FOUND: Cannot find module 'lib/PNG.ts' imported from nodes/wrongcase.ts",
        );
        expect(wrongTarget?.error).toBe("no module file Nodes/right.ts");
      }
    },
    HOST_TEST,
  );

  test(
    "a module Node loads as CommonJS is refused before it runs",
    async () => {
      const root = makeProject({
        "cjs/package.json": JSON.stringify({ type: "commonjs" }),
        "cjs/old.js": 'require("node:fs").writeFileSync(require("node:path").join(__dirname, "ran.txt"), "ran");\nmodule.exports = {};\n',
        "typeless/package.json": JSON.stringify({ name: "typeless" }),
        "typeless/old.ts": 'const fs = require("node:fs");\nmodule.exports = { fs };\n',
        "typeless/new.ts": NODE("", "detected"),
        "builders/old.cjs.js": "module.exports = {};\n",
      });
      const host = start(root);
      await host.initialize();
      const { modules } = await host.describe([{ path: "cjs/old.js" }, { path: "typeless/old.ts" }, { path: "typeless/new.ts" }]);
      expect(modules[0]?.error).toBe('cjs/old.js failed to import: cjs/old.js is not an ES module: set "type": "module" in package.json');
      expect(modules[1]?.error).toBe(
        'typeless/old.ts failed to import: typeless/old.ts is not an ES module: set "type": "module" in package.json',
      );
      expect(existsSync(join(root, "cjs", "ran.txt"))).toBe(false);
      // Without "type", Node detects ES module syntax: that module loads.
      expect(((modules[2]?.types as { attribute: string }[]) ?? []).map((type) => type.attribute)).toEqual(["detected"]);
      const built = await host.build("b", { path: "cjs/old.js", cwd: root });
      expect(built.error).toEqual({
        code: -32004,
        message: 'cjs/old.js failed to import: cjs/old.js is not an ES module: set "type": "module" in package.json',
      });
    },
    HOST_TEST,
  );

  test(
    "a project with no @grida/fx of its own imports the host's",
    async () => {
      const root = makeProject(
        {
          "nodes/a.ts": NODE('import { version } from "@grida/fx";\nexport const v = version;'),
          "nodes/missing.ts": 'import { nothing } from "@grida/fx-nothing";\nexport const n = nothing;\n',
        },
        { outside: true },
      );
      expect(existsSync(join(root, "node_modules"))).toBe(false);
      const host = start(root);
      await host.initialize();
      const { modules } = await host.describe([{ path: "nodes/a.ts" }, { path: "nodes/missing.ts" }]);
      expect((modules[0]?.types as { attribute: string }[]).map((type) => type.attribute)).toEqual(["uses"]);
      expect(labels(modules[0] as Record<string, unknown>)).toEqual(["nodes/a.ts"]);
      // Only @grida/fx itself falls back.
      expect(String(modules[1]?.error)).toStartWith(
        "nodes/missing.ts failed to import: ERR_MODULE_NOT_FOUND: Cannot find package '@grida/fx-nothing'",
      );
    },
    HOST_TEST,
  );

  test(
    "require finds the host's @grida/fx too; a subpath it does not export fails as the project's own import",
    async () => {
      const root = makeProject(
        {
          "lib/helper.cjs": 'module.exports = { version: require("@grida/fx").version };\n',
          "nodes/a.ts": NODE('import { createRequire } from "node:module";\nexport const v = createRequire(import.meta.url)("../lib/helper.cjs").version;'),
          "nodes/sub.ts": NODE('import { nothing } from "@grida/fx/nothing";\nexport const n = nothing;', "sub"),
        },
        { outside: true },
      );
      const host = start(root);
      await host.initialize();
      const { modules } = await host.describe([{ path: "nodes/a.ts" }, { path: "nodes/sub.ts" }]);
      expect((modules[0]?.types as { attribute: string }[]).map((type) => type.attribute)).toEqual(["uses"]);
      // The project's own error, which names the project's file, not the host's.
      expect(modules[1]?.error).toBe(
        "nodes/sub.ts failed to import: ERR_MODULE_NOT_FOUND: Cannot find package '@grida/fx' imported from nodes/sub.ts",
      );
      expect(String(modules[1]?.error)).not.toContain(builtHost().packageFolder);
    },
    HOST_TEST,
  );

  test(
    "a closure does not depend on what the session loaded before",
    async () => {
      const root = makeProject({
        // A shared registry that loads plugins lazily, when it is called.
        "lib/registry.ts": "export const load = (name: string) => import(`./plugins/${name}.ts`);\n",
        "lib/plugins/x.ts": 'export const x: string = "x";\n',
        // Its own top level loads one plugin itself: that one is the registry's.
        "lib/eager.ts": 'export const eager = (await import("./plugins/y.ts")).y;\n',
        "lib/plugins/y.ts": 'export const y = "y";\n',
        // A hashbang line stays the first line.
        "nodes/c.ts": `#!/usr/bin/env node
${NODE('import { load } from "../lib/registry.ts";\nimport { eager } from "../lib/eager.ts";\nexport const x = (await load("x")).x + eager;', "c")}`,
        "nodes/a.ts": NODE('import { load } from "../lib/registry.ts";\nimport { eager } from "../lib/eager.ts";\nexport const l = [load, eager];', "a"),
      });
      const a = ["lib/eager.ts", "lib/plugins/y.ts", "lib/registry.ts", "nodes/a.ts"];
      const c = ["lib/eager.ts", "lib/plugins/x.ts", "lib/plugins/y.ts", "lib/registry.ts", "nodes/c.ts"];
      for (const order of [["nodes/c.ts", "nodes/a.ts"], ["nodes/a.ts", "nodes/c.ts"], ["nodes/a.ts"]]) {
        const host = start(root);
        await host.initialize();
        const { modules } = await host.describe(order.map((path) => ({ path })));
        const closures = Object.fromEntries(modules.map((module) => [module.path, labels(module)]));
        expect(closures).toEqual(order.length === 1 ? { "nodes/a.ts": a } : { "nodes/a.ts": a, "nodes/c.ts": c });
      }
    },
    HOST_TEST,
  );

  test(
    "a file only located with import.meta.resolve is not in the closure",
    async () => {
      const root = makeProject({
        "nodes/a.ts": NODE('export const where = import.meta.resolve("../data/asset.json");'),
        "data/asset.json": "{}\n",
      });
      expect(labels(await closureOf(root, "nodes/a.ts"))).toEqual(["nodes/a.ts"]);
    },
    HOST_TEST,
  );

  test(
    "a file spelled with another Unicode normalization than its directory entry",
    async () => {
      const decomposed = "cafe\u0301.ts";
      const root = makeProject({
        "nodes/a.ts": NODE('import { cafe } from "../lib/caf\u00e9.ts";\nexport const c = cafe;'),
        [`lib/${decomposed}`]: "export const cafe = 1;\n",
      });
      const host = start(root);
      await host.initialize();
      const { modules } = await host.describe([{ path: "nodes/a.ts" }]);
      if (existsSync(join(root, "lib", "caf\u00e9.ts"))) {
        // A file system that finds either spelling (APFS).
        expect(modules[0]?.error).toBe(
          "nodes/a.ts failed to import: lib/caf\u00e9.ts is spelled with a different Unicode normalization on disk",
        );
      } else {
        expect(String(modules[0]?.error)).toStartWith("nodes/a.ts failed to import: ERR_MODULE_NOT_FOUND: ");
      }
    },
    HOST_TEST,
  );

  test(
    "a module Node loads as CommonJS is refused whatever loaded it first, and loads for others",
    async () => {
      const root = makeProject({
        "cjs/package.json": JSON.stringify({ type: "commonjs" }),
        "cjs/old.js": "module.exports = { old: 1 };\n",
        "nodes/uses_old.mts": NODE('import old from "../cjs/old.js";\nexport const o = old;', "uses_old"),
      });
      const refused = 'cjs/old.js failed to import: cjs/old.js is not an ES module: set "type": "module" in package.json';
      for (const order of [["nodes/uses_old.mts", "cjs/old.js"], ["cjs/old.js", "nodes/uses_old.mts"]]) {
        const host = start(root);
        await host.initialize();
        const { modules } = await host.describe(order.map((path) => ({ path })));
        const byPath = Object.fromEntries(modules.map((module) => [module.path, module]));
        expect(byPath["cjs/old.js"]?.error).toBe(refused);
        expect((byPath["nodes/uses_old.mts"]?.types as { attribute: string }[]).map((type) => type.attribute)).toEqual(["uses_old"]);
      }
    },
    HOST_TEST,
  );

  test(
    "a require Node answers from its cache counts for each requiring module, in either order",
    async () => {
      const REQUIRE = 'import { createRequire } from "node:module";\nconst require = createRequire(import.meta.url);\n';
      // Two sibling modules require one specifier from one folder: Node resolves the second from
      // its cache, without the hooks. The specifiers are computed, so only what Node loads counts.
      const sibling = (name: string) =>
        `import { node } from "@grida/fx";\n${REQUIRE}export const v = require("../data/" + "x.json");\n` +
        `export const ${name} = node({ name: "${name}" }, () => (require("../data/" + "x.json"), {}));\n` +
        `export const ${name}Late = node({ name: "${name}_late" }, () => (require("../data/" + "y.json"), {}));\n`;
      const root = makeProject({
        "data/x.json": "1\n",
        "data/y.json": "2\n",
        "nodes/c.ts": sibling("c"),
        "nodes/d.ts": sibling("d"),
        "lib/shared.cjs": "exports.s = 1;\n",
        "lib/a.cjs": 'exports.a = require("./" + "shared.cjs").s;\n',
        "lib/b.cjs": 'exports.b = require("./" + "shared.cjs").s;\n',
        "nodes/m.ts": NODE('import a from "../lib/a.cjs";\nexport const v = a;', "m"),
        "nodes/n.ts": NODE('import b from "../lib/b.cjs";\nexport const v = b;', "n"),
      });
      const expected = {
        "nodes/c.ts": ["data/x.json", "nodes/c.ts"],
        "nodes/d.ts": ["data/x.json", "nodes/d.ts"],
        "nodes/m.ts": ["lib/a.cjs", "lib/shared.cjs", "nodes/m.ts"],
        "nodes/n.ts": ["lib/b.cjs", "lib/shared.cjs", "nodes/n.ts"],
      };
      const order = Object.keys(expected);
      for (const described of [order, [...order].reverse()]) {
        expect(await closuresIn(root, described)).toEqual(expected);
        // The run-time guard: the same either order.
        const host = start(root);
        await host.initialize();
        await host.describe(described.map((path) => ({ path })));
        const [first, second] = described[0] === "nodes/c.ts" ? ["c", "d"] : ["d", "c"];
        expect(await runOf(host, `nodes/${first}.ts`, first, "r1")).toEqual({ outputs: {} });
        expect(await runOf(host, `nodes/${second}.ts`, second, "r2")).toEqual({ outputs: {} });
        // Each body that requires a file outside its closure fails, the second too, though Node
        // answers it from its cache.
        expect(await runOf(host, `nodes/${first}.ts`, `${first}Late`, "r3")).toBe(late("data/y.json"));
        expect(await runOf(host, `nodes/${second}.ts`, `${second}Late`, "r4")).toBe(late("data/y.json"));
        expect(await runOf(host, `nodes/${first}.ts`, first, "r5")).toEqual({ outputs: {} });
      }
    },
    HOST_TEST,
  );

  test(
    "a CommonJS helper's lazy require counts for the module whose top level calls it",
    async () => {
      const root = makeProject({
        "lib/helper.cjs": 'exports.load = (name) => require("./plugins/" + name + ".cjs");\n',
        "lib/plugins/x.cjs": "exports.x = 1;\n",
        "lib/plugins/y.cjs": "exports.y = 1;\n",
        "nodes/m.ts": NODE('import helper from "../lib/helper.cjs";\nexport const h = helper;', "m"),
        "nodes/n.ts":
          'import { node } from "@grida/fx";\nimport helper from "../lib/helper.cjs";\nexport const v = helper.load("x");\n' +
          'export const n = node({ name: "n" }, () => (helper.load("y"), {}));\n',
        // A CommonJS top level that calls a finished ES module's lazy require.
        "lib/registry.ts":
          'import { createRequire } from "node:module";\nconst require = createRequire(import.meta.url);\n' +
          "export const load = (name: string) => require(`./plugins/${name}.json`);\n",
        "lib/plugins/p.json": "1\n",
        "lib/eager.cjs": 'const { load } = require("./registry.ts");\nexports.p = load("p");\n',
        "nodes/e.ts": NODE('import eager from "../lib/eager.cjs";\nexport const v = eager.p;', "e"),
      });
      const expected = {
        "nodes/e.ts": ["lib/eager.cjs", "lib/plugins/p.json", "lib/registry.ts", "nodes/e.ts"],
        "nodes/m.ts": ["lib/helper.cjs", "nodes/m.ts"],
        "nodes/n.ts": ["lib/helper.cjs", "lib/plugins/x.cjs", "nodes/n.ts"],
      };
      const order = Object.keys(expected);
      for (const described of [order, [...order].reverse()]) {
        expect(await closuresIn(root, described)).toEqual(expected);
      }
      // In one host: what n's import loaded never fails m's run; n's body loading y.cjs fails n.
      const host = start(root);
      await host.initialize();
      expect(await runOf(host, "nodes/m.ts", "m", "r1")).toEqual({ outputs: {} });
      expect(await runOf(host, "nodes/n.ts", "n", "r2")).toBe(late("lib/plugins/y.cjs"));
      expect(await runOf(host, "nodes/m.ts", "m", "r3")).toEqual({ outputs: {} });
    },
    HOST_TEST,
  );

  test(
    "what a sibling's top level loads, awaiting or not, is that sibling's alone",
    async () => {
      const root = makeProject({
        "lib/registry.ts": "export const load = (name: string) => import(`./plugins/${name}.ts`);\n",
        "lib/plugins/p.ts": "export const p = 1;\n",
        "lib/plugins/q.ts": "export const q = 1;\n",
        // x awaits while y's top level calls the registry; z's top level calls it before y runs.
        "nodes/x.ts": NODE("await new Promise((done) => setTimeout(done, 50));", "x"),
        "nodes/y.ts": NODE('import { load } from "../lib/registry.ts";\nexport const v = await load("p");', "y"),
        "nodes/z.ts": NODE('import { load } from "../lib/registry.ts";\nexport const v = load("q");', "z"),
        "nodes/w.ts": NODE("", "w"),
        "nodes/m.ts": NODE('import "./x.ts";\nimport "./z.ts";\nimport "./w.ts";\nimport "./y.ts";', "m"),
      });
      const expected = {
        "nodes/m.ts": [
          "lib/plugins/p.ts",
          "lib/plugins/q.ts",
          "lib/registry.ts",
          "nodes/m.ts",
          "nodes/w.ts",
          "nodes/x.ts",
          "nodes/y.ts",
          "nodes/z.ts",
        ],
        "nodes/w.ts": ["nodes/w.ts"],
        "nodes/x.ts": ["nodes/x.ts"],
        "nodes/y.ts": ["lib/plugins/p.ts", "lib/registry.ts", "nodes/y.ts"],
        "nodes/z.ts": ["lib/plugins/q.ts", "lib/registry.ts", "nodes/z.ts"],
      };
      const order = Object.keys(expected);
      for (const described of [order, [...order].reverse()]) {
        expect(await closuresIn(root, described)).toEqual(expected);
      }
    },
    HOST_TEST,
  );

  test(
    "the marker binds no name a module can shadow, and keeps line one, byte order marks and hashbangs",
    async () => {
      const BOM = "\uFEFF";
      const HASHBANG = "#!/usr/bin/env node\n";
      const body = (name: string) => NODE('import { h } from "../lib/h.ts";\nexport const v = h;', name);
      const root = makeProject({
        "lib/h.ts": "export const h = 1;\n",
        "lib/model.ts": "export class Symbol {\n  name: string;\n  constructor(name: string) {\n    this.name = name;\n  }\n}\n",
        "nodes/class.ts": NODE("class Symbol {}\nexport const s = new Symbol();", "klass"),
        "nodes/const.ts": NODE("const Symbol = (name: string) => ({ name });\nexport const s = Symbol(\"star\");", "konst"),
        "nodes/function.ts": NODE("function Symbol() {}\nexport const s = Symbol;", "fn"),
        "nodes/global.ts": NODE("let globalThis = {};\nexport const g = globalThis;", "global"),
        "nodes/imported.ts": NODE('import { Symbol } from "../lib/model.ts";\nexport const s = new Symbol("star");', "imported"),
        "nodes/dependency.ts": NODE('import { Symbol } from "shapes";\nexport const s = new Symbol();', "dependency"),
        "node_modules/shapes/package.json": JSON.stringify({ name: "shapes", type: "module", exports: "./index.js" }),
        "node_modules/shapes/index.js": "export class Symbol {}\n",
        "nodes/bomhash.js": BOM + HASHBANG + body("bomhashjs"),
        "nodes/bomhash.mjs": BOM + HASHBANG + body("bomhashmjs"),
        "nodes/bomhash.ts": BOM + HASHBANG + body("bomhashts"),
        "nodes/bom.js": BOM + body("bomjs"),
        "nodes/hash.ts": HASHBANG + body("hashts"),
        // The body throws on line one; the import below it is hoisted.
        "nodes/one.ts":
          'export const one = node({ name: "one" }, () => { throw new Error("line one"); });\nimport { node } from "@grida/fx";\n',
      });
      const closures = await closuresIn(root, [
        "nodes/class.ts",
        "nodes/const.ts",
        "nodes/function.ts",
        "nodes/global.ts",
        "nodes/imported.ts",
        "nodes/dependency.ts",
        "nodes/bomhash.js",
        "nodes/bomhash.mjs",
        "nodes/bomhash.ts",
        "nodes/bom.js",
        "nodes/hash.ts",
      ]);
      expect(closures["nodes/imported.ts"]).toEqual(["lib/model.ts", "nodes/imported.ts"]);
      expect(closures["nodes/dependency.ts"]).toEqual(["nodes/dependency.ts"]);
      for (const name of ["bomhash.js", "bomhash.mjs", "bomhash.ts", "bom.js", "hash.ts"]) {
        expect(closures[`nodes/${name}`]).toEqual(["lib/h.ts", `nodes/${name}`]);
      }
      const host = start(root);
      await host.initialize();
      for (const [path, attribute] of [
        ["nodes/class.ts", "klass"],
        ["nodes/const.ts", "konst"],
        ["nodes/function.ts", "fn"],
        ["nodes/global.ts", "global"],
        ["nodes/imported.ts", "imported"],
        ["nodes/dependency.ts", "dependency"],
        ["nodes/bomhash.js", "bomhashjs"],
        ["nodes/bomhash.mjs", "bomhashmjs"],
        ["nodes/bomhash.ts", "bomhashts"],
      ] as const) {
        expect(await runOf(host, path, attribute, `r-${attribute}`)).toEqual({ outputs: {} });
      }
      const engine = new FakeEngine();
      host.engine = engine.answering;
      const answer = await host.request("run", runParams({ path: "nodes/one.ts", attribute: "one" }, { work_dir: engine.newWorkDir() }));
      const column = 'export const one = node({ name: "one" }, () => { throw '.length + 1;
      expect(String((answer.error?.data as { traceback?: unknown }).traceback)).toContain(`(nodes/one.ts:1:${column})`);
    },
    HOST_TEST,
  );

  test(
    "a file another module's import loaded never fails a run, even when the body only located it",
    async () => {
      const root = makeProject({
        "lib/x.ts": "export const x = 1;\n",
        "nodes/m.ts": NODE('export const where = import.meta.resolve("../lib/x.ts");', "m"),
        "nodes/n.ts": NODE('import { x } from "../lib/x.ts";\nexport const v = x;', "n"),
      });
      const host = start(root);
      await host.initialize();
      expect(await runOf(host, "nodes/m.ts", "m", "r1")).toEqual({ outputs: {} });
      expect(await runOf(host, "nodes/n.ts", "n", "r2")).toEqual({ outputs: {} });
      expect(await runOf(host, "nodes/m.ts", "m", "r3")).toEqual({ outputs: {} });
    },
    HOST_TEST,
  );

  test(
    "a module that is not a node module",
    async () => {
      const root = makeProject({ "nodes/a.ts": NODE("") });
      writeFileSync(join(root, "nodes", "notes.txt"), "text\n");
      const host = start(root);
      await host.initialize();
      const { modules } = await host.describe([{ path: "nodes/notes.txt" }]);
      expect(modules[0]?.error).toBe("cannot import nodes/notes.txt");
    },
    HOST_TEST,
  );
});

describe.skipIf(HOST_SKIP !== null)("literal specifiers count whether or not they run", () => {
  test(
    "a lazy load a module memoizes counts for every module that reaches it, in any order",
    async () => {
      const uses = (name: string) =>
        NODE(
          'import { getCodec } from "../lib/codec.ts";\nimport memo from "../lib/memo.cjs";\n' +
            `const { encode } = await getCodec();\nexport const v = [encode("${name}"), memo.get().v];`,
          name,
        );
      const root = makeProject({
        // The first caller's import() or require() loads the file; every later caller gets the
        // memo, and Node is never asked again.
        "lib/codec.ts":
          'let codec: Promise<typeof import("./codec-impl.ts")> | undefined;\n' +
          'export const getCodec = () => (codec ??= import("./codec-impl.ts"));\n',
        "lib/codec-impl.ts": "export const encode = (text: string) => text.toUpperCase();\n",
        "lib/memo.cjs": 'let impl;\nexports.get = () => (impl ??= require("./memo-impl.cjs"));\n',
        "lib/memo-impl.cjs": "exports.v = 1;\n",
        "nodes/a.ts": uses("a"),
        "nodes/b.ts": uses("b"),
        "nodes/c.ts":
          'import { node } from "@grida/fx";\nimport { getCodec } from "../lib/codec.ts";\nimport memo from "../lib/memo.cjs";\n' +
          'export const c = node({ name: "c" }, async () => {\n  (await getCodec()).encode("c");\n  memo.get();\n  return {};\n});\n',
      });
      const shared = ["lib/codec-impl.ts", "lib/codec.ts", "lib/memo-impl.cjs", "lib/memo.cjs"];
      for (const order of [["nodes/b.ts"], ["nodes/a.ts", "nodes/b.ts"], ["nodes/b.ts", "nodes/a.ts"]]) {
        const closures = await closuresIn(root, order);
        for (const path of order) {
          expect(closures[path]).toEqual([...shared, path]);
        }
      }
      // A body that calls the memoized loader passes, the first run that loads the file and every
      // later run the memo answers.
      const host = start(root);
      await host.initialize();
      for (const runId of ["r1", "r2", "r3"]) {
        expect(await runOf(host, "nodes/c.ts", "c", runId)).toEqual({ outputs: {} });
      }
    },
    HOST_TEST,
  );

  test(
    "in code that never runs; not in comments, strings, templates, regular expressions or types",
    async () => {
      const root = makeProject({
        "package.json": JSON.stringify({ type: "module", imports: { "#lib/*": "./lib/*" } }),
        "nodes/a.ts": [
          'import { node } from "@grida/fx";',
          'import { createRequire } from "node:module";',
          "const require = createRequire(import.meta.url);",
          "const helpers = { require: (path: string) => path };",
          "// Literal specifiers in functions nobody calls; what those files name counts too.",
          'export const never = () => import("../lib/never.ts");',
          'export const json = () => import("../data/opts.json", { with: { type: "json" } });',
          "export const plain = () => import(`../lib/plain-template.ts`);",
          'export const required = () => require("../lib/req.cjs");',
          'export const hashed = () => import("#lib/hashed.ts");',
          'export const nested = () => `x${String(import("../lib/nested.ts"))}y`;',
          'export const quote = /"/.test("a");',
          'export const afterRegex = () => import("../lib/after-regex.ts");',
          "export const ratio = (1) / 2 / 1;",
          'export const afterDivision = () => import("../lib/after-division.ts");',
          'export const shapes = () => import("@acme/shapes");',
          'export const dependency = () => import("dep");',
          "// Not code: comments, strings, templates and regular expressions.",
          '// import("../lib/commented.ts")',
          '/* require("../lib/block.cjs") */',
          "export const text = 'import(\"../lib/string.ts\")';",
          'export const tpl = `${1} import("../lib/template.ts") ${"x"}`;',
          'export const re = /import\\("..\\/lib\\/regex.ts"\\)/;',
          "// Types, which are erased; files that are not there as named; computed specifiers; a property.",
          'type Lazy = typeof import("../lib/typed.ts");',
          'export type Later = import("../lib/typed-later.ts").Later;',
          "export const lazy: Lazy | null = null;",
          'export const missing = () => import("../lib/missing.ts");',
          'export const misspelled = () => import("../lib/AFTER-regex.ts");',
          "export const computed = (name: string) => import(`../lib/${name}.ts`);",
          'export const added = (name: string) => require("../lib/" + name + ".cjs");',
          'export const member = helpers.require("../lib/member.cjs");',
          'export const a = node({ name: "a" }, () => ({}));',
          // A body that runs a literal import of its closure passes; a computed one does not.
          'export const runsLiteral = node({ name: "runs_literal" }, async () => {',
          "  await never();",
          "  required()();",
          "  return {};",
          "});",
          'export const runsComputed = node({ name: "runs_computed" }, async () => {',
          '  await computed("x");',
          "  return {};",
          "});",
        ].join("\n"),
        "lib/never.ts": 'import { deep } from "./never-deep.ts";\nexport const never = deep;\n',
        "lib/never-deep.ts": "export const deep = 1;\n",
        "data/opts.json": "{}\n",
        "lib/plain-template.ts": "export const plain = 1;\n",
        "lib/req.cjs": 'module.exports = () => require("./req-deep.cjs");\n',
        "lib/req-deep.cjs": "exports.deep = 1;\n",
        "lib/hashed.ts": "export const hashed = 1;\n",
        "lib/nested.ts": "export const nested = 1;\n",
        "lib/after-regex.ts": "export const after = 1;\n",
        "lib/after-division.ts": "export const after = 1;\n",
        "packages/shapes/package.json": JSON.stringify({ name: "@acme/shapes", type: "module", exports: "./index.ts" }),
        "packages/shapes/index.ts": "export const shape = 1;\n",
        "node_modules/dep/package.json": JSON.stringify({ name: "dep", type: "module", exports: "./index.js" }),
        "node_modules/dep/index.js": "export const dep = 1;\n",
        "lib/commented.ts": "export const no = 1;\n",
        "lib/block.cjs": "exports.no = 1;\n",
        "lib/string.ts": "export const no = 1;\n",
        "lib/template.ts": "export const no = 1;\n",
        "lib/regex.ts": "export const no = 1;\n",
        "lib/typed.ts": "export const no = 1;\n",
        "lib/typed-later.ts": "export type Later = number;\n",
        "lib/x.ts": "export const no = 1;\n",
        "lib/member.cjs": "exports.no = 1;\n",
      });
      mkdirSync(join(root, "node_modules", "@acme"), { recursive: true });
      symlinkSync(join(root, "packages", "shapes"), join(root, "node_modules", "@acme", "shapes"));
      const module = await closureOf(root, "nodes/a.ts", ["@acme/shapes"]);
      expect(labels(module)).toEqual([
        "@acme/shapes/index.ts",
        "data/opts.json",
        "lib/after-division.ts",
        "lib/after-regex.ts",
        "lib/hashed.ts",
        "lib/nested.ts",
        "lib/never-deep.ts",
        "lib/never.ts",
        "lib/plain-template.ts",
        "lib/req-deep.cjs",
        "lib/req.cjs",
        "nodes/a.ts",
      ]);
      const host = start(root);
      await host.initialize(["@acme/shapes"]);
      expect(await runOf(host, "nodes/a.ts", "runsLiteral", "r1")).toEqual({ outputs: {} });
      expect(await runOf(host, "nodes/a.ts", "runsComputed", "r2")).toBe(late("lib/x.ts"));
    },
    HOST_TEST,
  );

  test(
    "a createRequire from a folder, a package.json or a URL with a query counts for the module that calls it",
    async () => {
      const REQUIRE = 'import { createRequire } from "node:module";\n';
      const root = makeProject({
        "data/a.json": '{"v": 1}\n',
        "data/b.json": '{"v": 2}\n',
        "data/q.json": '{"v": 3}\n',
        "lib/c.cjs": "module.exports = 3;\n",
        "nodes/ctl.ts": NODE(`${REQUIRE}export const v = createRequire(import.meta.url)("../data/a.json");`, "ctl"),
        "nodes/dir.ts": NODE(`${REQUIRE}export const v = createRequire(new URL("./", import.meta.url))("../data/a.json");`, "dir"),
        "nodes/pkg.ts": NODE(
          `${REQUIRE}import { join } from "node:path";\nconst required = createRequire(join(process.cwd(), "package.json"));\n` +
            'export const v = [required("./data/b.json"), required("./lib/c.cjs")];',
          "pkg",
        ),
        // package.json is a module some other module loads, as JSON.
        "nodes/ver.ts": NODE('import pkg from "../package.json" with { type: "json" };\nexport const t = pkg.type;', "ver"),
        "nodes/viapkg.ts": NODE(`${REQUIRE}export const v = createRequire(new URL("../package.json", import.meta.url))("./data/b.json");`, "viapkg"),
        "nodes/query.ts": NODE('import { q } from "../lib/withq.ts?v=1";\nexport const v = q;', "query"),
        "lib/withq.ts": `${REQUIRE}export const q = createRequire(import.meta.url)("../data/q.json");\n`,
        "nodes/run.ts":
          `import { node } from "@grida/fx";\n${REQUIRE}const required = createRequire(new URL("../", import.meta.url));\n` +
          'export const run = node({ name: "run" }, () => (required("./lib/c.cjs"), {}));\n',
      });
      const expected = {
        "nodes/ctl.ts": ["data/a.json", "nodes/ctl.ts"],
        "nodes/dir.ts": ["data/a.json", "nodes/dir.ts"],
        "nodes/pkg.ts": ["data/b.json", "lib/c.cjs", "nodes/pkg.ts"],
        "nodes/query.ts": ["data/q.json", "lib/withq.ts", "nodes/query.ts"],
        "nodes/ver.ts": ["nodes/ver.ts", "package.json"],
        "nodes/viapkg.ts": ["data/b.json", "nodes/viapkg.ts"],
      };
      const order = Object.keys(expected);
      for (const described of [order, [...order].reverse()]) {
        expect(await closuresIn(root, described)).toEqual(expected);
      }
      // A body that requires through a folder's createRequire is held to its closure.
      const host = start(root);
      await host.initialize();
      expect(await runOf(host, "nodes/run.ts", "run", "r1")).toBe(late("lib/c.cjs"));
    },
    HOST_TEST,
  );

  test(
    "what a finished run left running is not charged to the next run",
    async () => {
      const root = makeProject({
        // Flushes on a timer, loading its writer on the first flush.
        "lib/telemetry.ts":
          "let timer: ReturnType<typeof setTimeout> | null = null;\n" +
          "export function note(writer: string): void {\n" +
          "  if (timer === null) {\n" +
          "    timer = setTimeout(async () => {\n" +
          "      timer = null;\n" +
          "      (await import(`./${writer}.ts`)).write();\n" +
          "    }, 300);\n" +
          "  }\n" +
          "}\n",
        "lib/serializer.ts": "export const write = () => {};\n",
        "nodes/a.ts":
          'import { node } from "@grida/fx";\nimport { note } from "../lib/telemetry.ts";\n' +
          'export const a = node({ name: "a" }, () => (note("serializer"), {}));\n' +
          'export const waits = node({ name: "waits" }, async () => {\n' +
          '  note("serializer");\n' +
          "  await new Promise((done) => setTimeout(done, 600));\n" +
          "  return {};\n" +
          "});\n",
        "nodes/b.ts":
          'import { node } from "@grida/fx";\nimport "../lib/telemetry.ts";\n' +
          'export const b = node({ name: "b" }, async () => {\n' +
          "  await new Promise((done) => setTimeout(done, 600));\n" +
          "  return {};\n" +
          "});\n",
      });
      const host = start(root);
      await host.initialize();
      // a's timer loads the writer while b runs: b loaded nothing.
      expect(await runOf(host, "nodes/a.ts", "a", "r1")).toEqual({ outputs: {} });
      expect(await runOf(host, "nodes/b.ts", "b", "r2")).toEqual({ outputs: {} });
      // A run whose own timer loads it is held to its closure.
      expect(await runOf(host, "nodes/a.ts", "waits", "r3")).toBe(late("lib/serializer.ts"));
      expect(await runOf(host, "nodes/b.ts", "b", "r4")).toEqual({ outputs: {} });
    },
    HOST_TEST,
  );

  test(
    "a user --conditions=require keeps every import edge",
    async () => {
      const root = makeProject({
        "nodes/a.ts": NODE('const name = "x";\nexport const v = (await import(`../lib/${name}.ts`)).x;'),
        "lib/x.ts": "export const x = 1;\n",
      });
      const host = start(root, { env: { ...process.env, NODE_OPTIONS: "--conditions=require" } });
      await host.initialize();
      const { modules } = await host.describe([{ path: "nodes/a.ts" }]);
      expect(labels(modules[0] as Record<string, unknown>)).toEqual(["lib/x.ts", "nodes/a.ts"]);
    },
    HOST_TEST,
  );

  test(
    "reading the stack leaves Error.stackTraceLimit as it was, even when prepareStackTrace is read-only",
    async () => {
      const root = makeProject({
        "lib/registry.ts": "export const load = (name: string) => import(`./plugins/${name}.ts`);\n",
        "lib/plugins/p.ts": "export const p = 1;\n",
        "nodes/x.ts":
          'import { node } from "@grida/fx";\nimport { load } from "../lib/registry.ts";\n' +
          'const user = (_error: Error, sites: unknown[]) => "frames: " + sites.length;\n' +
          'Object.defineProperty(Error, "prepareStackTrace", { value: user, writable: false, configurable: true });\n' +
          "Error.stackTraceLimit = 7;\n" +
          'await load("p");\n' +
          "const after = Error.stackTraceLimit;\n" +
          'export const x = node({ name: "x" }, () => {\n' +
          "  throw new Error(`limit ${after}`);\n" +
          "});\n",
      });
      const host = start(root);
      await host.initialize();
      expect(await runOf(host, "nodes/x.ts", "x", "r1")).toBe("limit 7");
    },
    HOST_TEST,
  );

  test(
    "a CommonJS module that left require.cache has finished: its later requires are made at call time",
    async () => {
      const root = makeProject({
        "lib/fresh.cjs": 'delete require.cache[__filename];\nexports.load = (name) => require("./plugins/" + name + ".cjs");\n',
        "lib/plugins/x.cjs": "exports.x = 1;\n",
        "lib/plugins/y.cjs": "exports.y = 1;\n",
        "nodes/m.ts": NODE('import fresh from "../lib/fresh.cjs";\nexport const f = fresh;', "m"),
        "nodes/n.ts":
          'import { node } from "@grida/fx";\nimport fresh from "../lib/fresh.cjs";\nexport const v = fresh.load("x");\n' +
          'export const n = node({ name: "n" }, () => (fresh.load("y"), {}));\n',
      });
      const expected = {
        "nodes/m.ts": ["lib/fresh.cjs", "nodes/m.ts"],
        "nodes/n.ts": ["lib/fresh.cjs", "lib/plugins/x.cjs", "nodes/n.ts"],
      };
      const order = Object.keys(expected);
      for (const described of [order, [...order].reverse()]) {
        expect(await closuresIn(root, described)).toEqual(expected);
      }
      const host = start(root);
      await host.initialize();
      expect(await runOf(host, "nodes/m.ts", "m", "r1")).toEqual({ outputs: {} });
      expect(await runOf(host, "nodes/n.ts", "n", "r2")).toBe(late("lib/plugins/y.cjs"));
      expect(await runOf(host, "nodes/n.ts", "n", "r3")).toBe(late("lib/plugins/y.cjs"));
    },
    HOST_TEST,
  );

  test(
    "a / after a keyword that is a property divides, so the literal import after it counts",
    async () => {
      const root = makeProject({
        "nodes/meter.ts": [
          'import { node } from "@grida/fx";',
          "const rate = (stats: { in: number }) => `received ${(stats.in / 1024).toFixed(1)} KiB/s`;",
          'export const meter = node({ name: "meter" }, async () => {',
          '  const { shout } = await import("../lib/format.ts");',
          "  return (shout(rate({ in: 2048 })), {});",
          "});",
        ].join("\n"),
        "lib/format.ts": "export const shout = (text: string): string => text.toUpperCase();\n",
      });
      expect(labels(await closureOf(root, "nodes/meter.ts"))).toEqual(["lib/format.ts", "nodes/meter.ts"]);
      const host = start(root);
      await host.initialize();
      expect(await runOf(host, "nodes/meter.ts", "meter", "r1")).toEqual({ outputs: {} });
    },
    HOST_TEST,
  );

  test(
    "the guard names a file the run loaded, never one only a literal in it names",
    async () => {
      const root = makeProject({
        "nodes/meter.ts": [
          'import { node } from "@grida/fx";',
          'export const meter = node({ name: "meter" }, async () => {',
          '  const name = "z-plugin";',
          "  await import(`../lib/${name}.ts`);",
          "  return {};",
          "});",
        ].join("\n"),
        // Loaded by the body's computed import. Its literal import is in a function nobody calls,
        // and lib/a-helper.ts sorts before it.
        "lib/z-plugin.ts": 'export const shout = (text: string) => text;\nexport const extra = () => import("./a-helper.ts");\n',
        "lib/a-helper.ts": "export const helper = 1;\n",
      });
      const host = start(root);
      await host.initialize();
      expect(await runOf(host, "nodes/meter.ts", "meter", "r1")).toBe(late("lib/z-plugin.ts"));
    },
    HOST_TEST,
  );

  test(
    "a body's literal import of a file spelled unlike its directory entries fails with the spelling",
    async () => {
      const root = makeProject({
        "nodes/meter.ts": [
          'import { node } from "@grida/fx";',
          'export const meter = node({ name: "meter" }, async () => {',
          '  await import("../lib/Format.ts");',
          "  return {};",
          "});",
        ].join("\n"),
        "lib/format.ts": "export const shout = (text: string) => text;\n",
      });
      const host = start(root);
      await host.initialize();
      const failed = String(await runOf(host, "nodes/meter.ts", "meter", "r1"));
      if (ignoresCase(root)) {
        // Node loads lib/format.ts as lib/Format.ts, which no import names.
        expect(failed).toBe(
          "the body loaded lib/Format.ts at run time, outside its source closure: lib/Format.ts is spelled lib/format.ts on disk",
        );
      } else {
        // Node finds no lib/Format.ts, and the body fails with its error, the root cut from it.
        expect(failed).toBe("Cannot find module 'lib/Format.ts' imported from nodes/meter.ts");
      }
    },
    HOST_TEST,
  );
});
