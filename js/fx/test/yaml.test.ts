import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { needsQuotes, toYaml } from "../src/yaml.js";
import { asJson, repoRoot, SLOW, temporaryFolder, testEngine } from "./support.js";

// The engine's own writer tests (crates/grida-fx-core/src/yaml/write.rs), with its expected text:
// this writer writes what the engine writes.

test("block layout", () => {
  const value = {
    fx: "lock/v1",
    nodes: {
      "nodes/acme.py#tint@1": "2166e237877fb0c822e9db9e5ec66af1ac0029648b722c00c5e566bacd251337",
    },
    list: [1, { a: 1, b: [true, null] }, [2, 3], [], {}],
    empty: {},
  };
  expect(toYaml(value)).toBe(
    "fx: lock/v1\n" +
      "nodes:\n" +
      "  nodes/acme.py#tint@1: 2166e237877fb0c822e9db9e5ec66af1ac0029648b722c00c5e566bacd251337\n" +
      "list:\n" +
      "  - 1\n" +
      "  - a: 1\n" +
      "    b:\n" +
      "      - true\n" +
      "      - null\n" +
      "  - - 2\n" +
      "    - 3\n" +
      "  - []\n" +
      "  - {}\n" +
      "empty: {}\n",
  );
});

test("top-level values", () => {
  expect(toYaml({})).toBe("{}\n");
  expect(toYaml([])).toBe("[]\n");
  expect(toYaml(null)).toBe("null\n");
  expect(toYaml("")).toBe('""\n');
  expect(toYaml("...")).toBe('"..."\n');
  expect(toYaml("---")).toBe('"---"\n');
  expect(toYaml([1, "a"])).toBe("- 1\n- a\n");
  expect(toYaml(7)).toBe("7\n");
  expect(toYaml("plain")).toBe("plain\n");
});

test("numbers in their canonical form", () => {
  const value = { a: 1.0, b: 0.5, c: 1e21, d: 1e-7, e: -2, f: 1152921504606847000, g: -0 };
  expect(toYaml(value)).toBe(
    "a: 1\nb: 0.5\nc: 1e+21\nd: 1e-7\ne: -2\nf: 1152921504606847000\ng: 0\n",
  );
});

test("digests and ambiguous strings are quoted", () => {
  expect(toYaml({ d: "123e4567" })).toBe('d: "123e4567"\n');
  for (const text of [
    "",
    "123e4567",
    "1e3",
    "on",
    "Off",
    "YES",
    "True",
    "null",
    "~",
    "true",
    "017",
    "1",
    "-1.5",
    "2026-10-05",
    "2026-10-05T10:00:00Z",
    "16:9",
    "1_000",
    ".inf",
    ".NaN",
    "0x1F",
    "9007199254740993",
    "1e400",
    "<<",
    "=",
  ]) {
    expect(needsQuotes(text)).toBe(true);
  }
  for (const text of [
    "a",
    "img-a@acme",
    "nodes/acme.py#tint@1",
    "a:b",
    "a#b",
    "y",
    "n",
    "0bad",
    "2026-1-5",
    "hello world",
    "é",
    "1.2.3",
    "a=b",
    "<<x",
  ]) {
    expect(needsQuotes(text)).toBe(false);
  }
});

test("keys are quoted when needed", () => {
  const value = { "<<": 1, on: 2, "1": 3, "": 4, "a: b": 5, plain: 6 };
  // JavaScript keeps integer-like keys first, so "1" leads.
  expect(toYaml(value)).toBe('"1": 3\n"<<": 1\n"on": 2\n"": 4\n"a: b": 5\nplain: 6\n');
});

// The resolver's own lists (crates/grida-fx-core/src/yaml/resolve.rs): every ambiguous form is
// quoted, and every lookalike is plain.

// prettier-ignore
const AMBIGUOUS = [
  "yes", "No", "ON", "oFF", "True", "FALSE", "nULL", "017", "-017", "01.5", "00.5", "01e3", "00",
  "0x1F", "-0x1F", "0X1F", "0xff_ff", "0o17", "0O17", "0b101", "+0b1", "0B101", "0x_", "1_000",
  "1_000.5", ".5_0", "1_", "0x1_F", "1:30", "16:9", "-1:30", "1:30.5", "10:00", "1:2:3", "1:30.",
  ".inf", "-.Inf", "+.INF", ".NaN", ".nan", ".iNf", "2026-10-05", "2026-10-05T10:00:00Z",
  "2026-1-5 9:30:00", "2026-10-05 10:00:00", "2026-10-05t10:00:00", "2026-10-05T10:00:00.5",
  "2026-10-05T10:00:00.", "2001-12-14 21:59:43.10 -5", "2026-10-05T10:00:00+09:00",
  "2026-10-05\t10:00:00", "2026-10-05  10:00:00 Z",
];

// prettier-ignore
const LOOKALIKES = [
  "0bad", "0ops", "0x", "0b12", "0o8", "0xg1", "+.NaN", "inf", "nan", ".infinity",
  "_", "snake_case", "v1_2", "_x1", "1_a", "yesterday", "online", "offset", "nullable", "Truth",
  "nope", "y", "n", "Y", "N", "2026-1-5", "1.2.3", "1024x1024", "3d", "a:b", "acme.test:8080",
  "1::30", "2026-10-05T10:00", "2026-10-05T10:00:00ZZ", "2026-10-05T1:00:0", "12026-10-05",
  "2026-100-05 10:00:00", ".", "+", "1e", "e3", ".e3", "1.5.5", "١٢", "yeſ",
];

test("ambiguous forms are quoted, lookalikes are not", () => {
  for (const text of AMBIGUOUS) {
    expect([text, needsQuotes(text)]).toEqual([text, true]);
  }
  for (const text of LOOKALIKES) {
    expect([text, needsQuotes(text)]).toEqual([text, false]);
  }
  // Plain-scalar syntax, beyond resolution: strings, but written quoted.
  for (const text of [
    "-.nan",
    "-inf",
    "-",
    "-leading",
    ":30",
    "1:",
    "a ",
    " a",
    "a: b",
    "a #b",
    "a\nb",
    "a\tb",
  ]) {
    expect([text, needsQuotes(text)]).toEqual([text, true]);
  }
});

test("quoted strings use JSON's escapes, and \\u for what a stream cannot hold", () => {
  expect(toYaml({ k: 'say "hi"\\' })).toBe('k: say "hi"\\\n');
  expect(toYaml({ k: '"hi" \\ x' })).toBe('k: "\\"hi\\" \\\\ x"\n');
  expect(toYaml({ k: "a\nb\tc\rd\be\ff" })).toBe('k: "a\\nb\\tc\\rd\\be\\ff"\n');
  expect(toYaml({ k: "\u0000\u007f\u0085\u2028\u2029\ufeff\ufffe\uffff" })).toBe(
    'k: "\\u0000\\u007f\\u0085\\u2028\\u2029\\ufeff\\ufffe\\uffff"\n',
  );
  expect(toYaml({ k: "é 日本 😀" })).toBe("k: é 日本 😀\n");
});

test("only FX values are written", () => {
  expect(() => toYaml({ a: Number.NaN })).toThrow('the value["a"] is NaN');
  expect(() => toYaml([Number.POSITIVE_INFINITY])).toThrow("not a number FX can hold");
  expect(() => toYaml({ a: [1, undefined] })).toThrow('the value["a"][1] is undefined');
  expect(() => toYaml({ a: new Date(0) })).toThrow("is a Date");
  expect(() => toYaml({ a: 1n })).toThrow("is a bigint");
  expect(() => toYaml({ a: "\ud800" })).toThrow("lone surrogate");
  expect(() => toYaml({ ["k".repeat(1025)]: 1 })).toThrow("longer than 1024");
  const deep: unknown[] = [];
  let at = deep;
  for (let i = 0; i < 600; i += 1) {
    const next: unknown[] = [];
    at.push(next);
    at = next;
  }
  expect(() => toYaml(deep)).toThrow("deeper than 512");
  // undefined members are left out, as JSON leaves them out.
  expect(toYaml({ a: 1, b: undefined })).toBe("a: 1\n");
});

// ------------------------------------------------------------------------------------------------
// Through the engine: what toYaml writes, the engine reads back as the same value.

/** The strings of the engine's round-trip test (write.rs, tricky_strings_round_trip). */
// prettier-ignore
const TRICKY = [
  "", " ", "  padded  ", " lead", "trail ", "a: b", "a:", "a :b", "a #b", "a#b", "#c", "- x", "-",
  "-x", "? x", "?x", ":x", ": x", ",x", "[x", "]x", "{x", "}x", "&x", "*x", "!x", "|x", ">x", "'x",
  '"x', "%x", "@x", "`x", "x,y", "x]", "x}", "x{y}", "x[0]", "a\nb", "a\n", "\n", "a\tb", "\t",
  "a\rb", "a\r\nb", "\u0000", "\u0001\u007f\u0080", "\u0085", "\u2028", "\u2029", "\ufeffx",
  "x\ufeff", "\ufffe\uffff", "back\\slash", 'quote"d', "it's", "'", '"', "é 日本 😀",
  "\u00a0x\u00a0", "<<", "...", "---", "...x", "--- x", "a ... b", "a --- b", "~", "null", "Null",
  "on", "yes", "y", "017", "0.5", "1e3", "123e4567", "2026-10-05", "16:9", "=", "a  b", "a   #b",
  "x:\ty", "x\t#y", "%YAML 1.2", "!!str", "&a", "*a", "a: ", "a ", "\\", "\u{1F600}", "\u{10FFFF}",
  "\ue000", "a\u200bb", "a\u0000b", ...AMBIGUOUS, ...LOOKALIKES,
];

/** A value the engine would read as a runtime marker, or as an expression (identity.md §3). */
function special(value: unknown): boolean {
  const text = JSON.stringify(value);
  return text.includes("${{") || /"(?:file|missing|failed|collection|pending)"/.test(text);
}

/** Every value of the accept vectors of spec/vectors/yaml. */
function vectorValues(): [string, unknown][] {
  const folder = join(repoRoot, "spec", "vectors", "yaml", "accept");
  return readdirSync(folder)
    .filter((name) => name.endsWith(".json"))
    .sort()
    .map((name): [string, unknown] => [name, JSON.parse(readFileSync(join(folder, name), "utf8"))])
    .filter(([, value]) => !special(value));
}

const engine = testEngine();

describe.skipIf(engine === null)("round trip through the engine", () => {
  test(
    "strings, keys, numbers and the yaml vectors read back as written",
    () => {
      const values: [string, unknown][] = [];
      for (const text of TRICKY) {
        values.push([`string ${JSON.stringify(text)}`, text]);
        values.push([`list of ${JSON.stringify(text)}`, [text, { [text]: [text] }]]);
        values.push([
          `key ${JSON.stringify(text)}`,
          { [text]: 1, nested: { [text]: { [text]: text } } },
        ]);
      }
      for (const number of [
        1.0,
        0.5,
        1e21,
        1e-7,
        -2,
        2 ** 60,
        -0,
        2 ** 53,
        -(2 ** 53),
        1e300,
        5e-324,
        0.1,
      ]) {
        values.push([`number ${number}`, number]);
      }
      values.push(...vectorValues());
      const steps: Record<string, unknown> = {};
      values.forEach(([, value], index) => {
        steps[`v${index}`] = {
          uses: "fx/image.generate@1",
          with: { prompt: "p", vars: { v: value } },
        };
      });
      const folder = temporaryFolder("yaml");
      try {
        mkdirSync(join(folder.path, "workflows"));
        writeFileSync(
          join(folder.path, "fx.yaml"),
          toYaml({ fx: "project/v1", routes: { "image.generate": "img-a@acme" } }),
        );
        writeFileSync(
          join(folder.path, "routes.yaml"),
          toYaml({
            fx: "routes/v1",
            routes: [
              {
                capability: "image.generate",
                route: "img-a@acme",
                price: { low_usd: 0.01, high_usd: 0.04 },
              },
            ],
          }),
        );
        writeFileSync(
          join(folder.path, "workflows", "round-trip.yaml"),
          toYaml({ fx: "workflow/v1", id: "round-trip", title: "Round trip", steps }),
        );
        const done = spawnSync(
          engine as string,
          ["expand", "workflows/round-trip.yaml", "--routes", "routes.yaml"],
          { cwd: folder.path, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 },
        );
        expect(done.stderr).toBe("");
        expect(done.status).toBe(0);
        const graph = JSON.parse(done.stdout) as {
          instances: { step: string; with: { vars: { v: unknown } } }[];
        };
        const read = new Map(
          graph.instances.map((instance) => [instance.step, instance.with.vars.v]),
        );
        values.forEach(([label, value], index) => {
          expect([label, read.get(`v${index}`)]).toEqual([label, asJson(value)]);
        });
      } finally {
        folder.cleanup();
      }
    },
    SLOW,
  );
});
