import { describe, expect, test } from "bun:test";
import { unzipSync } from "fflate";
import { OutputArchiveController, outputArchive, outputFiles, placedName, type OutputFile } from "../src/index";

const bytes = (text: string) => new TextEncoder().encode(text);
const hash = (data: Uint8Array) => new Bun.CryptoHasher("sha256").update(data).digest("hex");
const contents = new Map<string, Uint8Array>();
/** A recorded file value holding `text`. */
function file(text: string, kind = "image/png", key?: string) {
  const data = bytes(text);
  contents.set(hash(data), data);
  return { file: { digest: hash(data), kind, name: "ignored.bin", size: data.length, ...(key ? { key } : {}) } };
}
const locate = (digest: string) => `https://assets.example.com/artifacts/${digest}`;
const fetcher = (async (url: string) => {
  const data = contents.get(url.split("/").pop()!);
  return data ? new Response(data) : new Response(null, { status: 404 });
}) as unknown as typeof fetch;
async function unzip(files: OutputFile[]) {
  return Object.fromEntries(Object.entries(unzipSync(new Uint8Array(await (await outputArchive(files, { fetcher })).arrayBuffer()))).map(([name, data]) => [name, new TextDecoder().decode(data)]));
}

describe("run output archive", () => {
  test("names each file as the run folder's outputs/ does", () => {
    const files = outputFiles({
      hero: file("hero"),
      notes: file("{}", "json"),
      raw: file("raw", "file"),
      sheet: { list: [file("a"), file("b", "image/jpeg")] },
      looks: { collection: [["ada", file("ada")], ["hero.png", file("again")], ["a b/../c", file("deep")], ["", file("blank")]] },
      own: { list: [file("own", "image/png", "keyed")] },
      text: { value: "not a file" },
    }, locate);
    expect(files.map((item) => item.name)).toEqual([
      "hero.png", "notes.json", "raw",
      "sheet/0.png", "sheet/1.jpg",
      "looks/ada.png", "looks/hero.png", "looks/a_b/_/c.png", "looks/3.png",
      "own/keyed.png",
    ]);
    expect(files[0]).toEqual({ name: "hero.png", digest: hash(bytes("hero")), kind: "image/png", size: 4, url: locate(hash(bytes("hero"))) });
  });

  test("a single file in a list or a collection still gets a folder", () => {
    expect(outputFiles({ one: { list: [file("x")] }, keyed: { collection: [["k", file("x")]] } }, locate).map((item) => item.name)).toEqual(["one/0.png", "keyed/k.png"]);
  });

  test("cuts a long segment as the run folder does", async () => {
    const long = "k".repeat(300);
    expect(await placedName({ name: `looks/${long}.png`, kind: "image/png" })).toBe(`looks/${"k".repeat(234)}~8830571039dc4b0d.png`);
    expect(await placedName({ name: `${"é".repeat(200)}/a.png`, kind: "image/png" })).toBe(`${"é".repeat(119)}~df20b2aa6262e99e/a.png`);
  });

  test("zips the available files uncompressed, leaving out those with no URL", async () => {
    const files = outputFiles({ hero: file("hero"), looks: { collection: [["ada", file("ada")], ["bo", file("bo")]] } }, (digest) => digest === hash(bytes("bo")) ? null : locate(digest));
    expect(files.filter((item) => item.url === null).map((item) => item.name)).toEqual(["looks/bo.png"]);
    expect(await unzip(files)).toEqual({ "hero.png": "hero", "looks/ada.png": "ada" });
    const archive = new Uint8Array(await (await outputArchive(files, { fetcher })).arrayBuffer());
    // Local file header: compression method 0, stored.
    expect(archive[8] | (archive[9] << 8)).toBe(0);
  });

  test("a later file replaces an earlier one at the same name", async () => {
    expect(await unzip(outputFiles({ looks: { collection: [["a b", file("first")], ["a_b", file("second")]] } }, locate))).toEqual({ "looks/a_b.png": "second" });
  });

  test("refuses bytes that are not the recorded file", async () => {
    const [hero] = outputFiles({ hero: file("hero") }, locate);
    contents.set(hero.digest, bytes("evil"));
    await expect(outputArchive([hero], { fetcher })).rejects.toThrow("hero.png is not the file the run recorded.");
    contents.set(hero.digest, bytes("hero"));
    await expect(outputArchive([{ ...hero, url: locate("0".repeat(64)) }], { fetcher })).rejects.toThrow("hero.png could not be read (HTTP 404).");
  });

  test("refuses an archive too large for the tab before reading anything", async () => {
    let reads = 0;
    const counting = (async () => { reads += 1; return new Response(null); }) as unknown as typeof fetch;
    const files = outputFiles({ hero: file("hero") }, locate);
    await expect(outputArchive(files, { fetcher: counting, limit: 3 })).rejects.toThrow("more than this tab can put in one zip");
    expect(reads).toBe(0);
  });

  test("the controller saves <run>.zip and reports a failure", async () => {
    const saved: string[] = [];
    const controller = new OutputArchiveController({ fetcher, save: (_, name) => saved.push(name) });
    const files = outputFiles({ hero: file("hero") }, locate);
    const states: boolean[] = [];
    controller.subscribe(() => states.push(controller.getSnapshot().building));
    await controller.download(files, "run-01");
    expect(saved).toEqual(["run-01.zip"]);
    expect(states).toEqual([true, false]);
    await controller.download([{ ...files[0], url: locate("0".repeat(64)) }], "run-01");
    expect(controller.getSnapshot()).toEqual({ building: false, error: "hero.png could not be read (HTTP 404)." });
    expect(saved).toEqual(["run-01.zip"]);
  });
});
