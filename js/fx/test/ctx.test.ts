// What a node body receives (src/ctx.ts), over a fake channel to the engine: the cases of
// python/tests/test_ctx.py that apply to TypeScript bodies.

import { afterAll, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type Channel, type RunParams, RunContext } from "../src/ctx.js";
import { CallFailed, CeilingExceeded, EngineError, engineError, NodeFailure } from "../src/engine-errors.js";
import { InputFile, outputSourceOf } from "../src/files.js";
import { isModuleFile, kindOf } from "../src/kinds.js";
import { checkValue } from "../src/protocol.js";

const folder = realpathSync(mkdtempSync(join(tmpdir(), "grida-fx-ctx-")));
afterAll(() => rmSync(folder, { recursive: true, force: true }));
const store = join(folder, "store");
mkdirSync(store);
let works = 0;

const sha256 = (data: string | Uint8Array) => createHash("sha256").update(data).digest("hex");

/** A file ref of `data`, stored. */
function fileRef(data: string | Uint8Array, kind: string, name: string, key?: string) {
  const bytes = typeof data === "string" ? Buffer.from(data, "utf8") : Buffer.from(data);
  const digest = sha256(bytes);
  const path = join(store, digest);
  writeFileSync(path, bytes);
  return { digest, kind, size: bytes.length, name, ...(key === undefined ? {} : { key }), path, facts: { bytes: bytes.length, kind } };
}

type Answer = unknown | ((params: Record<string, unknown>) => unknown);

/** A channel that records what the context sends and answers as `answers` say. */
class FakeChannel implements Channel {
  readonly requests: [string, Record<string, unknown>][] = [];
  readonly notifications: [string, Record<string, unknown>][] = [];
  readonly answers: Record<string, Answer>;
  workDir = "";

  constructor(answers: Record<string, Answer> = {}) {
    this.answers = answers;
  }

  get methods(): string[] {
    return this.requests.map(([method]) => method);
  }

  async request(method: string, params: Record<string, unknown>): Promise<unknown> {
    try {
      checkValue({ run_id: "r1", ...params }, "", 1);
    } catch (error) {
      throw new TypeError(`${method}: ${(error as Error).message}`);
    }
    this.requests.push([method, params]);
    const answer = this.answers[method];
    if (typeof answer === "function") {
      return (answer as (params: Record<string, unknown>) => unknown)(params);
    }
    if (answer !== undefined) {
      return answer;
    }
    if (method === "file.put") {
      const data =
        typeof params.base64 === "string"
          ? Buffer.from(params.base64, "base64")
          : typeof params.work_path === "string"
            ? readFileSync(join(this.workDir, params.work_path))
            : Buffer.from(JSON.stringify(params.json, null, 1));
      return fileRef(data, String(params.kind), String(params.name));
    }
    return {};
  }

  notify(method: string, params: Record<string, unknown>): void {
    this.notifications.push([method, params]);
  }
}

function make(
  answers: Record<string, Answer> = {},
  over: Partial<Record<keyof RunParams, unknown>> = {},
): { ctx: RunContext; channel: FakeChannel; work: string } {
  works += 1;
  const work = join(folder, `work-${works}`);
  mkdirSync(work);
  const channel = new FakeChannel(answers);
  channel.workDir = work;
  const run = {
    run_id: "r1",
    instance: { id: "draw#1", path: "draw", step: "draw", key: null, take: [1, 3] },
    params: {},
    param_files: {},
    inputs: {},
    work_dir: work,
    resources: {},
    tools: {},
    calls: {},
    timeout_s: null,
    ...over,
  } as unknown as RunParams;
  return { ctx: new RunContext(run, channel), channel, work };
}

describe("what the run was given", () => {
  test("the instance", () => {
    const { ctx } = make();
    expect(ctx.instance).toEqual({ id: "draw#1", path: "draw", step: "draw", key: null, takes: [1, 3], take: 3 });
  });

  test("params get their files back", () => {
    const ref = fileRef("\x89PNG", "image/png", "ref.png");
    const other = fileRef("zip", "file/zip", "a.zip");
    const params = { prompt: "a cat", refs: [null, { deep: null }], "a/b": null, n: 3 };
    const { ctx } = make({}, { params, param_files: { "/refs/0": ref, "/refs/1/deep": other, "/a~1b": ref } });
    const got = ctx.params as Record<string, any>;
    expect(got.prompt).toBe("a cat");
    expect(got.n).toBe(3);
    expect(got.refs[0]).toBeInstanceOf(InputFile);
    expect(got.refs[0].digest).toBe(ref.digest);
    expect(got.refs[1].deep.kind).toBe("file/zip");
    expect(got["a/b"].name).toBe("ref.png");
    // The run's own params are not changed, and the same object comes back each time.
    expect(params.refs).toEqual([null, { deep: null }]);
    expect(ctx.params).toBe(got);
  });

  test.each([
    ["refs/0", 'param_files names "refs/0", which is not a pointer into params'],
    ["/missing/0", "param_files names /missing/0, which is not in params"],
    ["/refs/5", "param_files names /refs/5, which is not in params"],
  ])("a param file at %s that is not in params", (pointer, message) => {
    const { ctx } = make({}, { params: { refs: [null] }, param_files: { [pointer]: fileRef("x", "file", "x") } });
    expect(() => ctx.params).toThrow(message);
  });

  test("inputs by shape, a keyed collection in its order", () => {
    const one = fileRef("one", "text/plain", "one.txt");
    const a = fileRef("a", "text/plain", "a.txt");
    const b = fileRef("b", "text/plain", "b.txt");
    const { ctx } = make(
      {},
      {
        inputs: {
          image: one,
          refs: { list: [a, b] },
          lines: {
            collection: [
              ["sun", { ...b, key: "sun" }],
              ["10", a],
              ["2", b],
            ],
          },
        },
      },
    );
    const image = ctx.inputs.image as InputFile;
    expect(image).toBeInstanceOf(InputFile);
    expect([image.kind, image.digest, image.name, image.size, image.key, image.path]).toEqual([
      "text/plain",
      one.digest,
      "one.txt",
      3,
      null,
      one.path,
    ]);
    expect(image.facts).toEqual({ bytes: 3, kind: "text/plain" });
    expect((ctx.inputs.refs as InputFile[]).map((item) => item.name)).toEqual(["a.txt", "b.txt"]);
    const lines = ctx.inputs.lines as ReadonlyMap<string, InputFile>;
    expect(lines).toBeInstanceOf(Map);
    expect([...lines.keys()]).toEqual(["sun", "10", "2"]); // a plain object would read 2, 10, sun
    expect(lines.get("sun")?.key).toBe("sun");
    expect("missing" in ctx.inputs).toBe(false);
  });
});

describe("ctx.read", () => {
  test("reads are local; text is strict UTF-8 without one byte-order mark", async () => {
    const text = fileRef(Buffer.from("﻿﻿BOM text\r\nline2\r\n", "utf8"), "text/plain", "t");
    const data = fileRef('{"b": [1, 2.5], "a": null}', "json", "d.json");
    const a = fileRef("a", "text/plain", "a.txt");
    const { ctx, channel } = make({}, { inputs: { text, data, refs: { list: [a] } } });
    expect(await ctx.read.text("text")).toBe("﻿BOM text\r\nline2\r\n");
    expect(Buffer.from(await ctx.read.bytes("text")).subarray(0, 6)).toEqual(
      Buffer.from([0xef, 0xbb, 0xbf, 0xef, 0xbb, 0xbf]),
    );
    expect(await ctx.read.json("data")).toEqual({ b: [1, 2.5], a: null });
    expect(await ctx.read.text(ctx.inputs.data as InputFile)).toBe('{"b": [1, 2.5], "a": null}');
    expect(await (ctx.inputs.text as InputFile).text()).toBe("﻿BOM text\r\nline2\r\n");
    await expect(ctx.read.bytes("refs")).rejects.toThrow(new NodeFailure("input refs is not one file"));
    // An optional input the step left out.
    await expect(ctx.read.bytes("mask")).rejects.toThrow(new NodeFailure("input mask is absent"));
    await expect(ctx.read.bytes(3 as never)).rejects.toThrow("ctx.read takes an InputFile or an input's name, not a number");
    const bad = make({}, { inputs: { bad: fileRef(Buffer.from([0xff, 0xfe]), "text/plain", "bad.txt") } });
    await expect(bad.ctx.read.text("bad")).rejects.toThrow(TypeError);
    expect(channel.requests).toEqual([]);
  });
});

describe("ctx.out and work paths", () => {
  test("outputs are made, and nothing leaves the host until it is sent", () => {
    const { ctx, channel, work } = make();
    const data = ctx.out.bytes(new Uint8Array([0, 1]), "file/zip");
    expect(data.kind).toBe("file/zip");
    expect(outputSourceOf(data)).toEqual({ data: new Uint8Array([0, 1]) });
    const text = ctx.out.text("héllo");
    expect(text.kind).toBe("text/plain");
    expect(outputSourceOf(text)).toEqual({ data: new TextEncoder().encode("héllo") });
    expect(ctx.out.text("# h", "text/markdown").kind).toBe("text/markdown");
    const value = { b: [1, 2.5], a: null } as { b: number[]; a: null };
    const made = ctx.out.json(value);
    value.b.push(3); // the output keeps the value it was given
    expect(made.kind).toBe("json");
    expect(outputSourceOf(made)).toEqual({ json: { b: [1, 2.5], a: null } });
    expect(outputSourceOf(ctx.out.json(null))).toEqual({ json: null });
    expect(() => ctx.out.json({ x: Number.NaN })).toThrow(
      new TypeError("ctx.out.json takes a JSON value: at /x: NaN is not a JSON number"),
    );
    expect(() => ctx.out.json({ x: new Map() } as never)).toThrow("a Map is not a JSON value");
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0]);
    expect(ctx.out.png(png).kind).toBe("image/png");
    expect(() => ctx.out.png(new Uint8Array([1, 2, 3]))).toThrow("ctx.out.png takes an encoded PNG");
    expect(() => ctx.out.png("png" as never)).toThrow("ctx.out.png takes bytes (a Uint8Array), not a string");
    expect(() => ctx.out.text("lone \ud800")).toThrow("lone surrogates");
    const path = ctx.out.path("sub/caption.txt");
    expect(path).toBe(join(work, "out", "sub", "caption.txt"));
    expect(statSync(join(work, "out", "sub")).isDirectory()).toBe(true);
    expect(channel.requests).toEqual([]);
  });

  test("ctx.out.file: a work file leaves by its path, any other file's bytes are read now", () => {
    const { ctx } = make();
    const inside = ctx.out.path("caption.TXT");
    writeFileSync(inside, "words\n");
    const output = ctx.out.file(inside);
    expect(output.kind).toBe("text/plain");
    expect(outputSourceOf(output)).toEqual({ workPath: "out/caption.TXT" });
    expect(ctx.out.file(inside, "text/markdown").kind).toBe("text/markdown");
    const nested = ctx.workPath("a/b/mesh.glb");
    writeFileSync(nested, "glb");
    expect(outputSourceOf(ctx.out.file(nested))).toEqual({ workPath: "a/b/mesh.glb" });
    expect(ctx.out.file(nested).kind).toBe("model/gltf-binary");
    const outside = join(folder, "elsewhere.bin");
    writeFileSync(outside, "bytes");
    const read = ctx.out.file(outside);
    expect(read.kind).toBe("file");
    expect(outputSourceOf(read)).toEqual({ data: new Uint8Array(Buffer.from("bytes")) });
    expect(() => ctx.out.file(ctx.workPath("never-written.txt"))).toThrow("is not a file");
  });

  test("work paths stay in the work folder", () => {
    const { ctx, work } = make();
    expect(ctx.workPath("x/y.txt")).toBe(join(work, "x", "y.txt"));
    expect(ctx.workPath(".")).toBe(work);
    expect(ctx.workPath("..stays")).toBe(join(work, "..stays"));
    for (const name of ["../x", join(folder, "other.txt"), "a/../../b"]) {
      expect(() => ctx.workPath(name)).toThrow(new NodeFailure(`${name} is outside this node's work folder`));
    }
  });

  test("kinds follow the suffix table", () => {
    expect(kindOf("a.PNG")).toBe("image/png");
    expect(kindOf("a.jpeg")).toBe("image/jpeg");
    expect(kindOf("dir.d/a")).toBe("file");
    expect(kindOf("a.tar.gz")).toBe("file");
    expect(kindOf("x.mkv")).toBe("video/x-matroska");
    expect(kindOf("x.yml")).toBe("text/yaml");
    expect(kindOf(".png")).toBe("file");
  });

  test("module files by their exact suffix, as the engine reads them", () => {
    for (const path of ["nodes/a.ts", "nodes/a.mts", "nodes/a.js", "nodes/a.mjs", "lib/.d.ts", "lib/x.D.ts"]) {
      expect([path, isModuleFile(path)]).toEqual([path, true]);
    }
    for (const path of ["nodes/X.TS", "nodes/a.Mjs", "lib/x.d.ts", "lib/x.d.mts", "a.tsx", "a.cjs", "a.cts", ".ts", "a.", "a"]) {
      expect([path, isModuleFile(path)]).toEqual([path, false]);
    }
  });
});

describe("reporting", () => {
  test("facts are sent, then mirrored", async () => {
    const { ctx, channel } = make();
    await ctx.fact("score", 0.5);
    await ctx.fact("score", 0.75);
    await ctx.fact("sizes", [1, 2]);
    expect(channel.requests).toEqual([
      ["fact", { name: "score", value: 0.5 }],
      ["fact", { name: "score", value: 0.75 }],
      ["fact", { name: "sizes", value: [1, 2] }],
    ]);
    expect(ctx.facts).toEqual({ score: 0.75, sizes: [1, 2] });
    await expect(ctx.fact("bad", { x: Number.NaN })).rejects.toThrow(
      new TypeError("fact bad: at /x: NaN is not a JSON number"),
    );
    await expect(ctx.fact("nan", Number.NaN)).rejects.toThrow(new TypeError("fact nan: NaN is not a JSON number"));
    await expect(ctx.fact(3 as never, 1)).rejects.toThrow(new TypeError("a fact's name is a string, not a number"));
    expect(channel.requests.length).toBe(3);
    expect("bad" in ctx.facts).toBe(false);
  });

  test("a refused fact is not mirrored", async () => {
    const { ctx } = make({
      fact: () => {
        throw engineError(-32602, "cost_usd is the engine's");
      },
    });
    const refused = ctx.fact("cost_usd", 1);
    await expect(refused).rejects.toBeInstanceOf(EngineError);
    await expect(refused).rejects.toThrow("cost_usd is the engine's");
    expect(ctx.facts).toEqual({});
  });

  test("marks", async () => {
    const { ctx, channel } = make();
    await ctx.annotate({ shape: "box", box: [0.1, 0.2, 0.3, 0.4], label: "seam", color: "red", extra: 1 });
    await ctx.annotate({ label: "note only", color: undefined });
    await ctx.annotate({ shape: "points", points: [[0, 0], [1, 1]], closed: true, tag: "t" });
    expect(channel.requests).toEqual([
      ["annotate", { mark: { shape: "box", label: "seam", color: "red", box: [0.1, 0.2, 0.3, 0.4], extra: 1 } }],
      ["annotate", { mark: { label: "note only" } }],
      ["annotate", { mark: { shape: "points", tag: "t", points: [[0, 0], [1, 1]], closed: true } }],
    ]);
    expect(Object.keys(channel.requests[0]?.[1].mark as object)).toEqual(["shape", "label", "color", "box", "extra"]);
    expect(ctx.marks.map((mark) => mark.shape)).toEqual(["box", undefined, "points"]);
    await expect(ctx.annotate({ shape: "circle" as never })).rejects.toThrow(
      new NodeFailure("a mark's shape is one of point, points, box"),
    );
    for (const [shape, geometry] of [
      ["box", "box"],
      ["point", "at"],
      ["points", "points"],
    ] as const) {
      await expect(ctx.annotate({ shape })).rejects.toThrow(new NodeFailure(`a ${shape} mark needs ${geometry}`));
    }
    await expect(ctx.annotate({ label: "x", at: [Number.NaN, 0] })).rejects.toThrow(
      new TypeError("a mark: at /at/0: NaN is not a JSON number"),
    );
    expect(channel.requests.length).toBe(3);
    expect(ctx.marks.length).toBe(3);
  });

  test("progress is a notification", () => {
    const { ctx, channel } = make();
    ctx.progress("half way", 0.5);
    ctx.progress("going");
    expect(channel.notifications).toEqual([
      ["progress", { text: "half way", fraction: 0.5 }],
      ["progress", { text: "going" }],
    ]);
    expect(channel.requests).toEqual([]);
  });

  test("fail, state and signal", () => {
    const { ctx } = make();
    const failure = ctx.fail("on purpose");
    expect(failure).toBeInstanceOf(NodeFailure);
    expect(failure.message).toBe("on purpose");
    ctx.state.mesh = 1;
    expect(ctx.state).toEqual({ mesh: 1 });
    expect(ctx.signal.aborted).toBe(false);
    ctx.cancel(new Error("the engine cancelled run"));
    expect(ctx.signal.aborted).toBe(true);
    expect((ctx.signal.reason as Error).message).toBe("the engine cancelled run");
  });

  test("what a failure keeps: facts without cost_usd, marks, each only when it is I-JSON", async () => {
    const { ctx } = make();
    await ctx.fact("kept", 1);
    await ctx.annotate({ label: "why" });
    expect(ctx.kept()).toEqual({ facts: { kept: 1 }, marks: [{ label: "why" }] });
    ctx.facts.cost_usd = 3;
    ctx.facts.broken = Number.NaN;
    expect(ctx.kept()).toEqual({ marks: [{ label: "why" }] });
  });
});

describe("prompts and capabilities", () => {
  test("a prompt renders a declared resource, files in its variables as file values", async () => {
    const image = fileRef("img", "image/png", "i.png");
    const { ctx, channel } = make(
      { "prompt.render": (params: Record<string, unknown>) => ({ text: `rendered ${String(params.path)}` }) },
      { resources: { "prompts/seam.md": "/abs/prompts/seam.md" }, inputs: { image } },
    );
    await expect(ctx.prompt("prompts/other.md")).rejects.toThrow(
      new NodeFailure("prompts/other.md is not one of this node's declared resources"),
    );
    const made = ctx.out.text("made");
    const text = await ctx.prompt("prompts/seam.md", { who: "W", n: 3, pic: ctx.inputs.image as InputFile, made: [made, made] });
    expect(text).toBe("rendered prompts/seam.md");
    expect(channel.requests[0]).toEqual([
      "file.put",
      { base64: Buffer.from("made").toString("base64"), kind: "text/plain", name: "prompt/file" },
    ]);
    const madeDigest = sha256("made");
    expect(channel.requests.slice(1)).toEqual([
      [
        "prompt.render",
        {
          path: "prompts/seam.md",
          variables: { who: "W", n: 3, pic: { file: image.digest }, made: [{ file: madeDigest }, { file: madeDigest }] },
        },
      ],
    ]);
  });

  test("a capability request carries files; each output is stored once, a work file each time", async () => {
    const image = fileRef("img", "image/png", "i.png");
    const a = fileRef("a", "image/png", "a.png");
    const picture = fileRef("answer", "image/png", "out.png");
    const answer = { key: "c".repeat(64), cached: false, cost_usd: 0.04, files: { image: picture }, data: { revised: "x" } };
    const { ctx, channel } = make({ capability: answer }, { inputs: { image, refs: { list: [a] } } });
    const mask = ctx.out.bytes(Buffer.from("mask"), "image/png");
    const written = ctx.out.path("guide.png");
    writeFileSync(written, "guide");
    const guide = ctx.out.file(written);
    const data = ctx.out.json({ k: 1 });
    const result = await ctx.imageEdit({
      image: ctx.inputs.image as InputFile,
      mask,
      references: ctx.inputs.refs as readonly InputFile[],
      nested: { a: [mask, guide], data, keep: { file: "a.png" } },
      prompt: "paint",
    });
    expect(channel.methods).toEqual(["file.put", "file.put", "file.put", "capability"]);
    const puts = channel.requests.filter(([method]) => method === "file.put").map(([, params]) => params);
    expect(puts).toEqual([
      { base64: Buffer.from("mask").toString("base64"), kind: "image/png", name: "request/file" },
      { work_path: "out/guide.png", kind: "image/png", name: "request/file" },
      { json: { k: 1 }, kind: "json", name: "request/file" },
    ]);
    const maskDigest = sha256("mask");
    expect(channel.requests[3]?.[1]).toEqual({
      capability: "image.edit",
      request: {
        image: { file: image.digest },
        mask: { file: maskDigest },
        references: [{ file: a.digest }],
        nested: {
          a: [{ file: maskDigest }, { file: sha256("guide") }],
          data: { file: sha256(JSON.stringify({ k: 1 }, null, 1)) },
          keep: { file: "a.png" },
        },
        prompt: "paint",
      },
    });
    expect(result.image.digest).toBe(picture.digest);
    expect(Buffer.from(await result.image.bytes()).toString()).toBe("answer");
    expect(result.files.get("image")?.name).toBe("out.png");
    expect([result.costUsd, result.cached, result.key]).toEqual([0.04, false, "c".repeat(64)]);
    expect(result.json).toEqual({ revised: "x" });
    expect(result.data).toEqual({ revised: "x" });
    expect(() => result.audio).toThrow(new CallFailed("the call returned no audio"));
    expect(() => result.video).toThrow(CallFailed);
    // A second call stores an output again only when it is a work file (it may have changed).
    await ctx.capability("image.generate", { mask, guide });
    expect(channel.methods.slice(4)).toEqual(["file.put", "capability"]);
    expect(channel.requests[4]?.[1].work_path).toBe("out/guide.png");
    const structured = await make({
      capability: { key: "d".repeat(64), cached: true, cost_usd: 0, files: {}, data: { json: { caption: "a kite" } } },
    }).ctx.structuredGenerate({ prompt: "p" });
    expect(structured.json).toEqual({ caption: "a kite" });
    expect(structured.cached).toBe(true);
    expect(() => structured.image).toThrow("the call returned no image");
  });

  test("the sugar names its capability; the engine's errors reach the body by code", async () => {
    const seen: string[] = [];
    const { ctx } = make({
      capability: (params: Record<string, unknown>) => {
        seen.push(String(params.capability));
        if (params.capability === "structured.generate") {
          throw engineError(-32014, "run ceiling reached", { needed_usd: 1, remaining_usd: 0 });
        }
        return { key: "d".repeat(64), cached: true, cost_usd: 0, files: {}, data: null };
      },
    });
    await ctx.imageGenerate({ prompt: "p" });
    try {
      await ctx.structuredGenerate({ prompt: "p" });
      throw new Error("not refused");
    } catch (error) {
      expect(error).toBeInstanceOf(CeilingExceeded);
      expect((error as CeilingExceeded).code).toBe(-32014);
      expect((error as CeilingExceeded).data).toEqual({ needed_usd: 1, remaining_usd: 0 });
      expect((error as Error).message).toBe("run ceiling reached");
    }
    expect(seen).toEqual(["image.generate", "structured.generate"]);
    await expect(ctx.capability("image.generate", { p: new Map() as never })).rejects.toThrow(
      new TypeError("capability: at /request/p: a Map is not a JSON value"),
    );
    await expect(ctx.capability("image.generate", { refs: [undefined] as never })).rejects.toThrow(
      new TypeError("capability: at /request/refs/0: undefined is not a JSON value"),
    );
  });

  test("a request member whose value is undefined is left out, as JSON.stringify leaves it out", async () => {
    const answer = { key: "c".repeat(64), cached: true, cost_usd: null, files: {}, data: null };
    const { ctx, channel } = make({ capability: answer });
    const size: string | undefined = undefined;
    await ctx.imageGenerate({ prompt: "p", size, nested: { keep: 1, drop: undefined } });
    await ctx.fact("facts", { kept: true, dropped: undefined });
    const sent = channel.requests.map(([method, params]) => [method, JSON.parse(JSON.stringify(params))]);
    expect(sent).toEqual([
      ["capability", { capability: "image.generate", request: { prompt: "p", nested: { keep: 1 } } }],
      ["fact", { name: "facts", value: { kept: true } }],
    ]);
  });
});

describe("external programs", () => {
  const sh = "/bin/sh";

  test("tool handles", () => {
    const { ctx, work } = make({}, { tools: { sh, blender: null } });
    expect(() => ctx.tool("ffmpeg")).toThrow(new NodeFailure("ffmpeg is not one of this node's declared tools"));
    expect(() => ctx.tool("blender")).toThrow(new NodeFailure("blender is not installed (see grida-fx doctor)"));
    const handle = ctx.tool("sh");
    expect(handle.executable).toBe(sh);
    expect(handle.workDir).toBe(work);
  });

  test("a program runs in the work folder, stdin closed, its output captured", async () => {
    const { ctx, work } = make({}, { tools: { sh } });
    const shell = ctx.tool("sh");
    const done = await shell.run(["-c", "pwd; echo err >&2"]);
    expect(done.status).toBe(0);
    expect(realpathSync(done.stdout.trim())).toBe(work);
    expect(done.stderr).toBe("err\n");
    const elsewhere = join(folder, "elsewhere");
    mkdirSync(elsewhere, { recursive: true });
    expect(realpathSync((await shell.run(["-c", "pwd"], { cwd: elsewhere })).stdout.trim())).toBe(elsewhere);
    await expect(shell.run(["-c", "echo out; echo '  bad  ' >&2; exit 3"])).rejects.toThrow(
      new NodeFailure("sh exited 3: bad"),
    );
    await expect(shell.run(["-c", "echo only stdout; exit 4"])).rejects.toThrow(new NodeFailure("sh exited 4: only stdout"));
    expect((await shell.run(["-c", "exit 5"], { check: false })).status).toBe(5);
    expect((await shell.run(["-c", "kill -9 $$"], { check: false })).status).toBe(-9);
    const replaced = await shell.run(["-c", 'echo "[$HOME][$ONLY]"'], { env: { ONLY: "this" } });
    expect(replaced.stdout).toBe("[][this]\n");
    expect((await shell.run(["-c", "cat"])).stdout).toBe("");
    const long = await shell
      .run(["-c", `printf '%2500s' '' | tr ' ' a >&2; printf END >&2; exit 2`])
      .catch((error: Error) => error.message);
    expect(String(long)).toStartWith("sh exited 2: ");
    expect(String(long).slice("sh exited 2: ".length)).toHaveLength(2000);
    expect(String(long)).toEndWith("END");
  });

  /** Whether the process `pid` ends within 5 seconds. */
  async function ends(pid: number): Promise<boolean> {
    for (let tries = 0; tries < 100; tries += 1) {
      try {
        process.kill(pid, 0);
      } catch {
        return true;
      }
      await new Promise((done) => setTimeout(done, 50));
    }
    return false;
  }

  /** The pid a program wrote to `file`, once it did. */
  async function pidIn(file: string): Promise<number> {
    for (let tries = 0; tries < 200; tries += 1) {
      try {
        const text = readFileSync(file, "utf8").trim();
        if (text) {
          return Number(text);
        }
      } catch {
        // not yet
      }
      await new Promise((done) => setTimeout(done, 25));
    }
    throw new Error(`no pid in ${file}`);
  }

  test("timeoutS is a number of seconds above 0; null or left out, there is none", async () => {
    const { ctx } = make({}, { tools: { sh } });
    const shell = ctx.tool("sh");
    for (const timeoutS of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, "5"]) {
      await expect(shell.run(["-c", "exit 0"], { timeoutS: timeoutS as never })).rejects.toThrow(
        new TypeError(`timeoutS is a number of seconds above 0, or null, not ${String(timeoutS === "5" ? "'5'" : timeoutS)}`),
      );
    }
    expect((await shell.run(["-c", "sleep 0.2; echo done"], { timeoutS: null })).stdout).toBe("done\n");
    // Longer than a timer can wait at once (about 24.8 days): armed again, never fired at once.
    expect((await shell.run(["-c", "sleep 0.2; echo late"], { timeoutS: 30 * 24 * 3600 })).stdout).toBe("late\n");
  });

  test("ctx.signal ends a program with what it started; the run's end ends what is left", async () => {
    const { ctx } = make({}, { tools: { sh } });
    const pidFile = join(folder, "aborted.pid");
    const running = ctx.tool("sh").run(["-c", `sleep 30 & echo $! > ${pidFile}; wait`]);
    const child = await pidIn(pidFile);
    const reason = new Error("the engine cancelled run");
    ctx.cancel(reason);
    await expect(running).rejects.toBe(reason);
    expect(await ends(child)).toBe(true);
    // Once aborted, a program does not start.
    await expect(ctx.tool("sh").run(["-c", "exit 0"])).rejects.toBe(reason);

    const other = make({}, { tools: { sh } });
    const leftFile = join(folder, "left.pid");
    const left = other.ctx.tool("sh").run(["-c", `sleep 30 & echo $! > ${leftFile}; wait`], { check: false });
    const grandchild = await pidIn(leftFile);
    other.ctx.endTools();
    expect((await left).status).toBe(-9);
    expect(await ends(grandchild)).toBe(true);
  });

  test("at its timeout a program ends with what it started", async () => {
    const { ctx } = make({}, { tools: { sh } });
    const pidFile = join(folder, "child.pid");
    const started = Date.now();
    await expect(
      ctx.tool("sh").run(["-c", `sleep 30 & echo $! > ${pidFile}; wait`], { timeoutS: 1.5 }),
    ).rejects.toThrow(new NodeFailure("sh ran past 1.5 seconds"));
    expect(Date.now() - started).toBeLessThan(10_000);
    const child = Number(readFileSync(pidFile, "utf8").trim());
    let alive = true;
    for (let tries = 0; tries < 100 && alive; tries += 1) {
      try {
        process.kill(child, 0);
        await new Promise((done) => setTimeout(done, 50));
      } catch {
        alive = false;
      }
    }
    expect(alive).toBe(false);
  });
});
