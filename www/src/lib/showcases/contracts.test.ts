import { describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { deliveryBase, deliveryUrl, showcaseLock } from "./catalog";
import { auditPortableMetadata, parseCharacter, parseDemo, parseFace, parseLock, parseParallax, parseRelease, parseSheet, parseTerrainLookup, parseWorld, safePath } from "./contracts";
import { loadShowcaseJson } from "./load";

const fixtures = new URL("../../../content/showcases/", import.meta.url);
async function fixture(path: string): Promise<unknown> { return JSON.parse(await readFile(new URL(path, fixtures), "utf8")); }
async function demo(slug: string) { return fixture(`manifests/${showcaseLock.workflows[slug].demo.manifest.path}`); }
describe("locked showcase adapters", () => {
  test("every frozen demo is supported and its resources are confined", async () => {
    for (const slug of Object.keys(showcaseLock.workflows)) {
      const resources = parseDemo(slug as Parameters<typeof parseDemo>[0], await demo(slug));
      expect(resources.length).toBeGreaterThan(0);
      for (const path of resources) expect(safePath(path)).toBe(path);
    }
  });
  test("unknown kinds and unsafe resource paths cannot render", async () => {
    const data = await demo("parallax") as Record<string, unknown>;
    expect(() => parseParallax({ ...data, kind: "parallax-demo-v2" })).toThrow("Unsupported");
    expect(() => parseParallax({ ...data, layers: [{ id: "a", file: "../private.png", offset_y: 0, order: 0, parallax: .5 }] })).toThrow("confined");
    for (const value of ["/etc/passwd", "a/../b", "a\\b", "https://host/x", "x?sig=1", "x#y", "x/%2e%2e/y"]) expect(() => safePath(value)).toThrow();
  });
  test("references, geometry and required state sets remain semantic checks", async () => {
    const world = await demo("world") as Record<string, unknown>;
    const parsed = parseWorld(world);
    parsed.entities[0].relationships.push({ other_entity_id: "missing", other_display_name: "Missing", summary: "Missing" });
    expect(() => parseWorld(parsed)).toThrow("references");
    const character = parseCharacter(await demo("character-3d")); character.models = character.models.filter(model => model.stage !== "result");
    expect(() => parseCharacter(character)).toThrow("stages");
    const face = parseFace(await fixture("manifests/portrait-motion/assets/face-manifest.json")); face.feather_already_baked = false;
    expect(() => parseFace(face)).toThrow("semantics");
    const sheet = parseSheet(await fixture("manifests/ui-kit/assets/button_rect_validation.json")); sheet.cells.pop();
    expect(() => parseSheet(sheet)).toThrow("button states");
    const lookup = parseTerrainLookup(await fixture("manifests/terrain/assets/lookup.json")); lookup.lookup[Object.keys(lookup.lookup)[0]] = [12, 0];
    expect(() => parseTerrainLookup(lookup)).toThrow("lookup");
  });
  test("all named icons and the canonical reference consumed by the UI are mandatory", async () => {
    const icons = parseSheet(await fixture("manifests/ui-kit/assets/preview_icons_validation.json"));
    const missing = structuredClone(icons); delete missing.cells[0].glyph;
    expect(() => parseSheet(missing)).toThrow("icon sheet");
    const changed = structuredClone(icons); changed.cells[0].glyph = "unrecognized";
    expect(() => parseSheet(changed)).toThrow("icon sheet");
    const character = parseCharacter(await demo("character-3d"));
    character.images = character.images.map(image => ({ ...image, source_label: image.source_label.replace("canonical", "source") }));
    expect(() => parseCharacter(character)).toThrow("canonical reference");
  });
  test("catalog hashes cannot silently identify a different release or recording", () => {
    const changed = structuredClone(showcaseLock); changed.release.path = "showcases/latest/manifest.json";
    expect(() => parseLock(changed)).toThrow("digest");
    const second = structuredClone(showcaseLock); second.workflows.parallax.recordings[0].path = "runs/latest/manifest.json";
    expect(() => parseLock(second)).toThrow("unpinned");
  });
  test("release metadata must belong to its exact file inventory", async () => {
    const release = parseRelease(await fixture(showcaseLock.release.fixture));
    release.demos.parallax.manifest.sha256 = "a".repeat(64);
    expect(() => parseRelease(release)).toThrow("inventory");
  });
  test("known local coordinates and signed credentials are refused", () => {
    for (const value of [{ nested: ["/Users/alice/private.png"] }, { url: "https://host/x?X-Amz-Signature=abc" }, { authorization: "Bearer x" }]) expect(() => auditPortableMetadata(value)).toThrow();
    expect(() => auditPortableMetadata({ prompt: "An original meadow in warm light", file: "assets/meadow.png" })).not.toThrow();
  });
  test("runtime loading verifies exact downloaded bytes before parsing", async () => {
    const bytes = await readFile(new URL("manifests/parallax/manifest.json", fixtures));
    const originalFetch = globalThis.fetch;
    try {
      globalThis.fetch = (() => Promise.resolve(new Response(bytes))) as unknown as typeof fetch;
      expect((await loadShowcaseJson("parallax", "https://assets.example/showcase", "manifest.json", parseParallax)).layers.length).toBe(3);
      globalThis.fetch = (() => Promise.resolve(new Response(Buffer.concat([bytes, Buffer.from(" ")])))) as unknown as typeof fetch;
      await expect(loadShowcaseJson("parallax", "https://assets.example/showcase", "manifest.json", parseParallax)).rejects.toThrow("integrity");
    } finally { globalThis.fetch = originalFetch; }
  });
});


test("delivery configuration supports a local preview or explicit CDN without embedded credentials", () => {
  const previous = process.env.FX_SHOWCASE_ASSET_BASE;
  try {
    delete process.env.FX_SHOWCASE_ASSET_BASE;
    expect(deliveryBase()).toBe("/fx");
    process.env.FX_SHOWCASE_ASSET_BASE = "https://assets.example.com/root///";
    expect(deliveryBase()).toBe("https://assets.example.com/root");
    expect(deliveryUrl("runs/example/manifest.json")).toBe("https://assets.example.com/root/runs/example/manifest.json");
    for (const base of ["//assets.example.com", "https://user:secret@assets.example.com", "https://assets.example.com?token=secret", "https://assets.example.com#fragment", "file:///private/assets"]) {
      process.env.FX_SHOWCASE_ASSET_BASE = base;
      expect(() => deliveryBase()).toThrow("Invalid showcase delivery base");
    }
  } finally {
    if (previous === undefined) delete process.env.FX_SHOWCASE_ASSET_BASE;
    else process.env.FX_SHOWCASE_ASSET_BASE = previous;
  }
});
