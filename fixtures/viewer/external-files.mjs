/** Original, provider-free file-activation fixture. Source modules deliberately cannot be
 * imported: opening current files must use recorded references, never author execution. */
import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { deflateSync } from "node:zlib";

const hash = bytes => createHash("sha256").update(bytes).digest("hex");

function crc32(bytes) {
  let value = 0xffffffff;
  for (const byte of bytes) {
    value ^= byte;
    for (let bit = 0; bit < 8; bit++) value = (value >>> 1) ^ ((value & 1) ? 0xedb88320 : 0);
  }
  return (value ^ 0xffffffff) >>> 0;
}

function chunk(name, bytes) {
  const tag = Buffer.from(name), length = Buffer.alloc(4), checksum = Buffer.alloc(4);
  length.writeUInt32BE(bytes.length); checksum.writeUInt32BE(crc32(Buffer.concat([tag, bytes])));
  return Buffer.concat([length, tag, bytes, checksum]);
}

/** A PNG drawn from three original flat-color panels, synthesized without an image service. */
function image() {
  const width = 192, height = 128, pixels = Buffer.alloc((width * 3 + 1) * height);
  const colors = [[232, 119, 101], [247, 216, 133], [97, 153, 148]];
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
    const color = colors[Math.floor(x / 64)];
    pixels.set(color, y * (width * 3 + 1) + 1 + x * 3);
  }
  const header = Buffer.alloc(13); header.writeUInt32BE(width); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", header), chunk("IDAT", deflateSync(pixels)), chunk("IEND", Buffer.alloc(0))]);
}

export function externalFilesFixture(directory) {
  const project = join(directory, "project"), run = join(project, "runs", "recorded");
  for (const path of [join(project, "workflows"), join(project, "nodes"), join(run, "files", "draw")]) mkdirSync(path, { recursive: true });
  writeFileSync(join(project, "fx.yaml"), "fx: project/v1\n");
  writeFileSync(join(project, "workflows", "main.yaml"), "# Current editable root workflow.\nfx: workflow/v1\nid: sample\nsteps:\n  draw:\n    uses: ./nodes/render.py#render\n");
  writeFileSync(join(project, "workflows", "child.yaml"), "# Current editable imported workflow.\nfx: workflow/v1\nid: child\nsteps: {}\n");
  const code = "raise RuntimeError('File resolution must never import this source')\n";
  writeFileSync(join(project, "nodes", "render.py"), code);
  writeFileSync(join(project, "builder.py"), "raise RuntimeError('Opening a builder must never run it')\n");
  const bytes = image(), digest = hash(bytes);
  const file = { digest, kind: "image/png", name: "original-panels.png", size: bytes.length };
  const plan = {
    kind: "fx-graph-v1", plan: "a".repeat(64), workflow: { id: "sample", title: "External file fixture", file: "workflows/main.yaml" },
    types: { "./nodes/render.py#render": { identity: "nodes/render.py#render@1", source: { files: { "nodes/render.py": hash(code) }, resources: {} } } },
    instances: [{ id: "draw#1", path: "draw", step: "draw", take: [1], uses: "./nodes/render.py#render", type: "nodes/render.py#render@1",
      state: "planned", reads: [], with: {}, routes: {}, identity: null, key: null, phase: 1, price: { low_usd: 0, high_usd: 0 },
      waiting_on: [], judged_by: [], needs: [], judges: null, view: false }],
    scopes: [{ id: "scope:child#1", parent: null, kind: "workflow", path: "child", step: "child", take: [1], title: "Imported workflow",
      source: "workflows/child.yaml", ports: { inputs: {}, outputs: [] }, input_bindings: [], output_bindings: [], nodes: ["draw#1"], pending: [] }],
    steps: { draw: { title: "Original image", uses: "./nodes/render.py#render" } }, inputs: {}, pending: [], stand_in: true,
    estimate: { low_usd: 0, high_usd: 0, ceiling_usd: null },
  };
  writeFileSync(join(run, "plan.json"), JSON.stringify(plan));
  const encoded = { file };
  const events = [
    { event: "run_started", stand_in: true },
    { event: "node_started", id: "draw#1", path: "draw", step: "draw", take: [1], uses: "./nodes/render.py#render", with: {}, reads: [] },
    { event: "node_finished", id: "draw#1", path: "draw", outputs: { image: encoded }, cache: "hit", duration_ms: 3 },
    { event: "run_finished", ok: true, charged_usd: 0, outputs: { result: encoded } },
  ].map(event => ({ kind: "fx-run-events-v1", plan: plan.plan, invocation_id: "offline-file-fixture", offset_ms: 0, ...event }));
  writeFileSync(join(run, "events.jsonl"), events.map(event => JSON.stringify(event)).join("\n") + "\n");
  const artifactPath = join(run, "files", "draw", "image.png");
  writeFileSync(artifactPath, bytes);
  const builderPlan = join(directory, "builder-plan.json");
  writeFileSync(builderPlan, JSON.stringify({ ...plan, workflow: { ...plan.workflow, file: "builder.py:build" } }));
  return { project, run, builderPlan, artifactPath, digest, file, bytes };
}
