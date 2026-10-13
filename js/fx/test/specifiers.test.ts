// The literal specifiers a module names (src/specifiers.ts), which count for a TypeScript source
// closure whether or not the code runs: imports, exports from, import() and require() with a
// literal, and nothing in comments, strings, templates, regular expressions or computed calls.

import { describe, expect, test } from "bun:test";
import { literalSpecifiers } from "../src/specifiers.js";

const named = (code: string) => literalSpecifiers(code).map(({ kind, specifier }) => `${kind} ${specifier}`);

describe("literal specifiers", () => {
  test("every form of import and export from", () => {
    const code = [
      'import a from "./a.ts";',
      'import { b, c as d } from "./b.ts";',
      'import { "e-f" as g } from "./e.ts";',
      'import * as h from "./h.ts";',
      'import i, { j } from "./i.ts";',
      'import k, * as l from "./k.ts";',
      'import from from "./from.ts";',
      'import "./side.ts";',
      'import data from "./data.json" with { type: "json" };',
      'export * from "./star.ts";',
      'export * as ns from "./ns.ts";',
      'export { m, n as o } from "./m.ts";',
      "export { p };",
      'export const q = 1, r = "./not.ts";',
      "export default {};",
    ].join("\n");
    expect(named(code)).toEqual([
      "import ./a.ts",
      "import ./b.ts",
      "import ./e.ts",
      "import ./h.ts",
      "import ./i.ts",
      "import ./k.ts",
      "import ./from.ts",
      "import ./side.ts",
      "import ./data.json",
      "import ./star.ts",
      "import ./ns.ts",
      "import ./m.ts",
    ]);
  });

  test("import() and require() with a literal, wherever they are", () => {
    const code = [
      "export const a = () => import('./a.ts');",
      'export const b = async () => (await import("./b.json", { with: { type: "json" } })).default;',
      "export const c = () => import(`./c.ts`);",
      'export const d = () => require("./d.cjs");',
      "export const e = [...require('./e.cjs')];",
      "if (false) { require(\"./f.cjs\"); }",
      'export const g = () => `x${String(import("./g.ts"))}y${`${require("./h.cjs")}`}`;',
      'export const i = "\\x2e/i.ts", j = () => import("\\x2e/j.ts");',
    ].join("\n");
    expect(named(code)).toEqual([
      "import ./a.ts",
      "import ./b.json",
      "import ./c.ts",
      "require ./d.cjs",
      "require ./e.cjs",
      "require ./f.cjs",
      "import ./g.ts",
      "require ./h.cjs",
      "import ./j.ts",
    ]);
  });

  test("nothing computed, nothing that is not a call of import or require", () => {
    const code = [
      "export const a = (name: string) => import(name);",
      "export const b = (name: string) => import(`./${name}.ts`);",
      'export const c = (name: string) => require("./" + name);',
      'export const d = () => import("./d" + ".ts");',
      'export const e = helpers.require("./e.cjs");',
      'export const f = helpers?.require("./f.cjs");',
      'export const g = this.#require("./g.cjs");',
      'export const h = require.resolve("./h.cjs");',
      'export const i = import.meta.resolve("./i.ts");',
      "export const url = import.meta.url;",
      'const options = { import: "./j.ts", require: "./k.cjs" };',
      'export const myrequire = myimport("./l.ts") + requireIt("./m.cjs");',
    ].join("\n");
    expect(named(code)).toEqual([]);
  });

  test("not in comments, strings, templates or regular expressions", () => {
    const code = [
      '// import "./line.ts";',
      '/* require("./block.cjs")',
      '   import("./block.ts") */',
      "export const s = 'import(\"./single.ts\")';",
      'export const t = "require(\'./double.cjs\')";',
      'export const u = `import("./template.ts") ${1} require("./template.cjs")`;',
      'export const v = /import\\("\\.\\/regex.ts"\\)/g;',
      'export const w = /[/"]/.test("x");',
      'export const after = () => import("./after-regex.ts");',
      "export const ratio = (4) / 2 / 1;",
      "export const halves = [4][0] / 2;",
      "let n = 1; n++ / 2;",
      'export const afterDivision = () => import("./after-division.ts");',
      "export function f() { return /'/.test(\"'\"); }",
      'export const afterReturn = () => import("./after-return.ts");',
    ].join("\n");
    expect(named(code)).toEqual(["import ./after-regex.ts", "import ./after-division.ts", "import ./after-return.ts"]);
  });

  test("a keyword that is a property is an operand: a / after it divides", () => {
    const code = [
      "const rate = (stats) => `received ${(stats.in / 1024).toFixed(1)} KiB/s`;",
      'export const a = () => import("./a.ts");',
      "const half = (opts, b) => opts.default / 2 + `/${b}`;",
      'export const b = () => import("./b.ts");',
      "const rest = (x) => [x.of / 2, x?.new / 2, x.delete / 2, x.return / 2, x.typeof / 2] + `/${x}`;",
      'export const c = () => import("./c.ts");',
      "export const end = `${1}`;",
    ].join("\n");
    expect(named(code)).toEqual(["import ./a.ts", "import ./b.ts", "import ./c.ts"]);
  });

  test("after the head of an if, while, for or with a / starts a regular expression", () => {
    const code = [
      "if (ok) /'/.test(s); import(\"./a.ts\");",
      "while (more()) /`/.exec(s);",
      'export const b = () => import("./b.ts");',
      "for await (const x of y) /'/.test(x), import(\"./c.ts\");",
      "with (scope) /`/.test(s);",
      'export const d = () => import("./d.ts");',
      // A call or a method named like a statement word is an operand.
      "const w = (arr) => arr.with(0, 1) / 2 + `/${1}`;",
      'export const e = () => import("./e.ts");',
      "const f = (a) => a.if(1) / 2 + a.for(2) / 3 + `/${a}`;",
      'export const g = () => import("./g.ts");',
      "export const end = `${1}`;",
    ].join("\n");
    expect(named(code)).toEqual([
      "import ./a.ts",
      "import ./b.ts",
      "import ./c.ts",
      "import ./d.ts",
      "import ./e.ts",
      "import ./g.ts",
    ]);
  });

  test("a clause's braces may hold any number of names", () => {
    const names = Array.from({ length: 3000 }, (_, index) => `a${index} as b${index}`).join(", ");
    expect(named(`export { ${names} } from "./many.ts";\nimport { ${names} } from "./more.ts";\n`)).toEqual([
      "import ./many.ts",
      "import ./more.ts",
    ]);
  });

  test("a namespace exported under a string name", () => {
    expect(named('export * as "tools" from "./g.ts";\nexport * as "a-b" from "./h.ts";\n')).toEqual([
      "import ./g.ts",
      "import ./h.ts",
    ]);
    expect(named('export * "tools" from "./no.ts";\nimport * as "x" "./no.ts";\n')).toEqual([]);
  });

  test("a long line of slashes that are divisions, and of open escapes, reads in linear time", () => {
    // About 2 MB on one line of each shape, and how many specifiers each repetition names. Each `/`
    // after `}` starts a regular expression that the class `[` leaves open to the line's end, and
    // one that follows it on the line is still found; each `\u{` escape has no `}`; each `import`
    // starts a clause that never ends.
    const shapes: [string, number][] = [
      ['x={}/1+"[";', 0],
      ['y=o.default/2+"[";', 0],
      ['z="\\u{12";', 0],
      ["w=String.raw`\\u{12`;", 0],
      ['x={}/1+"[";y=/\'/.test(s);import("./a.ts");', 1],
      // Not code, but words that each start an import clause.
      ["import ", 0],
      ["import {import, ", 0],
    ];
    for (const [shape, each] of shapes) {
      const times = Math.ceil(2_000_000 / shape.length);
      const code = shape.repeat(times);
      const started = performance.now();
      const found = literalSpecifiers(code);
      const elapsed = performance.now() - started;
      expect(elapsed).toBeLessThan(1000);
      expect(found.length).toBe(each * times);
    }
    expect(named('import("\\u{00000002e}/x.ts"); import("\\u{2e}/y.ts"); import("\\u{110000}");')).toEqual([
      "import ./x.ts",
      "import ./y.ts",
      "import u{110000}",
    ]);
  });

  test("a hashbang, a byte order mark, and code after an unterminated string", () => {
    expect(named('\ufeff#!/usr/bin/env node\nimport "./a.ts";\n')).toEqual(["import ./a.ts"]);
    expect(named('#!/usr/bin/env node "./no.ts"\nimport "./b.ts";\n')).toEqual(["import ./b.ts"]);
    expect(named("const broken = 'open\nimport \"./c.ts\";\n")).toEqual(["import ./c.ts"]);
  });
});
