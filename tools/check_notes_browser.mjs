#!/usr/bin/env bun
/** Real recorded notes in the bundled viewer: rendering, scope, SDK opt-in and native source
 * opening. Original provider-free source lives in fixtures/viewer; no network or paid work. */
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = fileURLToPath(new URL("../", import.meta.url));
const engine = process.env.GRIDA_FX_BIN ?? resolve(root, "target/debug/grida-fx");
const python = process.env.GRIDA_FX_PYTHON ?? resolve(root, "python/.venv/bin/python");
const directory = mkdtempSync(resolve(tmpdir(), "fx-notes-browser-"));
const env = { ...process.env, GRIDA_FX_NETWORK: "off", GRIDA_FX_DISABLE_DOTENV: "1", GRIDA_FX_PYTHON: python };
for (const name of ["GG_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY", "FAL_KEY", "TRIPO_API_KEY", "ELEVENLABS_API_KEY"]) delete env[name];
let server, browser;
try {
  const fixture = Bun.spawnSync([python, resolve(root, "tools/check_notes.py"), "--command", engine, "--output", directory], { env, cwd: root });
  assert.equal(fixture.exitCode, 0, fixture.stderr.toString() || fixture.stdout.toString());
  const report = JSON.parse(fixture.stdout.toString());
  const sdk = await Bun.build({ entrypoints: [resolve(root, "js/fx/src/viewer/client.ts")], target: "browser", format: "esm" });
  assert.equal(sdk.success, true, String(sdk.logs));
  const sdkCode = await sdk.outputs[0].text();
  server = Bun.spawn([engine, "view", "--run", report.run, "--port", "0"], { env, cwd: report.project, stdout: "pipe", stderr: "pipe" });
  const reader = server.stdout.getReader(); let stdout = "", url;
  const timeout = setTimeout(() => server.kill(), 15_000);
  try {
    while (!url) {
      const { value, done } = await reader.read();
      if (done) throw new Error("Notes viewer exited before readiness: " + stdout);
      stdout += new TextDecoder().decode(value);
      url = /^http:\/\/127\.0\.0\.1:\d+\//m.exec(stdout)?.[0];
    }
  } finally { clearTimeout(timeout); reader.releaseLock(); }
  browser = await chromium.launch({ headless: true, ...(process.env.FX_CHROMIUM_EXECUTABLE ? { executablePath: process.env.FX_CHROMIUM_EXECUTABLE } : {}) });
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
  const errors = [], external = [];
  const origin = new URL(url).origin;
  await context.route("**/*", route => {
    const target = route.request().url();
    if (new URL(target).origin === origin || target.startsWith("data:")) return route.continue();
    external.push(target); return route.abort();
  });
  const page = await context.newPage();
  page.on("pageerror", error => errors.push(error.message));
  await page.route("**/note-client.js", route => route.fulfill({ body: sdkCode, contentType: "text/javascript" }));
  await page.goto(url);
  await page.waitForSelector('.fx-canvas-note[data-note-id="note:root:overview"]');
  const nextPaint = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const geometry = () => page.locator(".fx-canvas-note, .fx-canvas-frame").evaluateAll(cards => cards.map(card => {
    const box = card.querySelector(":scope > rect");
    return { id: card.getAttribute("data-note-id") ?? card.getAttribute("data-frame-id"), transform: card.getAttribute("transform"), x: box.getAttribute("x"), y: box.getAttribute("y"), width: box.getAttribute("width"), height: box.getAttribute("height") };
  }));
  await nextPaint();
  mkdirSync(resolve(root, "output/playwright"), { recursive: true });
  await page.screenshot({ path: resolve(root, "output/playwright/workflow-notes-initial.png") });
  const overview = page.locator('.fx-canvas-note[data-note-id="note:root:overview"]');
  const literal = page.locator('.fx-canvas-note[data-note-id="note:root:literal"]');
  const longDocument = page.locator('.fx-canvas-note[data-note-id="note:root:long_document"]');
  const noteSize = card => card.locator(":scope > rect").evaluate(box => ({ width: box.width.baseVal.value, height: box.height.baseVal.value }));
  const overviewSize = await noteSize(overview), literalSize = await noteSize(literal), longSize = await noteSize(longDocument);
  assert.equal(overviewSize.width, literalSize.width);
  assert.equal(longSize.width, literalSize.width);
  assert.ok(literalSize.height < overviewSize.height, "Short notes shrink to their content");
  assert.ok(overviewSize.height > 240, "Ordinary Markdown grows beyond the old fixed excerpt");
  assert.ok(longSize.height > overviewSize.height, "Only genuinely long documents reach the canvas bound");
  for (const card of [overview, literal]) {
    assert.equal(await card.getAttribute("data-collapsed"), "false");
    assert.equal(await card.locator(".fx-canvas-note-more").count(), 0);
    const fits = await card.locator("foreignObject").evaluate(frame => {
      const body = frame.firstElementChild;
      return body.scrollHeight <= frame.height.baseVal.value + 1 && getComputedStyle(body).maskImage === "none";
    });
    assert.equal(fits, true, "An ordinary note is fully visible without a fade");
  }
  assert.equal(await longDocument.getAttribute("data-collapsed"), "true");
  assert.match(await longDocument.locator(".fx-canvas-note-more").textContent(), /Read full note/);
  const rootRects = await page.locator('.fx-canvas-note[data-note-id^="note:root:"]').evaluateAll(cards => cards.map(card => {
    const { x, y, width, height } = card.getBoundingClientRect(); return { x, y, width, height };
  }));
  for (let a = 0; a < rootRects.length; a++) for (let b = a + 1; b < rootRects.length; b++) {
    const left = rootRects[a], right = rootRects[b];
    assert.ok(left.x + left.width <= right.x || right.x + right.width <= left.x || left.y + left.height <= right.y || right.y + right.height <= left.y, "Measured root notes must not overlap");
  }
  const groupSizes = await page.locator(".fx-canvas-frame").evaluateAll(frames => frames.map(frame => {
    const box = frame.querySelector(":scope > rect"); return { width: box.getAttribute("width"), height: box.getAttribute("height") };
  }));
  assert.equal(groupSizes.length, 2);
  assert.deepEqual(groupSizes[0], groupSizes[1], "Repeated group frames share the same measured tracks");
  const groupNotes = await page.locator(".fx-canvas-frame.fx-deck-card").evaluateAll(frames => frames.map(frame => ({
    id: frame.getAttribute("data-frame-id"), note: frame.querySelector("[data-note-id]").getAttribute("data-note-id"),
  })));
  const frontFrame = () => page.locator(".fx-canvas-frame.fx-deck-card").last().getAttribute("data-frame-id");
  const restingFrame = await frontFrame();
  const backNote = groupNotes.find(frame => frame.id !== restingFrame);
  await page.evaluate(note_id => window.fx_viewer.present({ action: "reveal_note", note_id }, { includeNotes: true }), backNote.note);
  assert.equal(await frontFrame(), backNote.id, "Inspecting a group note raises its containing member");
  await page.getByRole("button", { name: "Close details", exact: true }).click();
  assert.equal(await frontFrame(), restingFrame, "Clearing a group note restores the resting deck representative");
  const originalGeometry = await geometry();
  // Use the real camera controls, then pan to compare ordinary, short and long cards at once.
  const currentWidth = (await literal.boundingBox()).width;
  const zoomDelta = -Math.log(0.9 / (currentWidth / literalSize.width)) / 0.01;
  await page.mouse.move(720, 500);
  await page.keyboard.down("Control");
  await page.mouse.wheel(0, zoomDelta);
  await page.keyboard.up("Control");
  await nextPaint();
  const noteBand = await overview.boundingBox();
  await page.mouse.wheel(noteBand.x - 110, noteBand.y - 100);
  await nextPaint();
  await page.screenshot({ path: resolve(root, "output/playwright/workflow-notes-content-sized.png") });
  await page.getByRole("button", { name: "Fit workflow", exact: true }).click();
  assert.equal(await overview.locator("h1").innerText(), "Reference to variations");
  assert.equal(await overview.locator("img").count(), 0);
  await overview.click();
  const details = page.locator(".fx-note-details");
  await details.waitFor();
  assert.deepEqual(await geometry(), originalGeometry, "Selection does not resize notes or group tracks");
  assert.match(await details.innerText(), /The expression above is documentation/);
  assert.equal(await page.evaluate(() => window.fx_note_script_executed), undefined);
  assert.equal(await page.evaluate(() => window.fx_viewer.getContext().selection), null);
  const selection = await page.evaluate(() => window.fx_viewer.getContext({ includeNotes: true }).selection);
  assert.deepEqual(selection, { kind: "note", note_id: "note:root:overview", scope_id: null });
  await page.evaluate(async () => {
    const { ViewerClient } = await import("/note-client.js");
    const legacy = new ViewerClient({ targetWindow: window, origin: location.origin });
    const modern = new ViewerClient({ targetWindow: window, origin: location.origin, includeNotes: true });
    if ((await legacy.getContext()).selection !== null) throw new Error("Legacy client received note selection");
    if ((await modern.getContext()).selection?.kind !== "note") throw new Error("Opted-in client lost note selection");
    legacy.dispose(); modern.dispose();
  });
  await page.evaluate(() => {
    window.noteFiles = [];
    window.detachNoteFiles = window.fx_viewer.handleExternalFile(reference => { window.noteFiles.push(reference); });
  });
  await details.locator(".fx-source-link").waitFor();
  await details.locator(".fx-source-link").click();
  const reference = await page.evaluate(() => window.noteFiles[0]);
  assert.equal(reference.target.source, "notes/overview.md");
  assert.equal(JSON.stringify(reference).includes(report.project), false);
  const resolved = Bun.spawnSync([engine, "files", "resolve", report.run, "--reference", JSON.stringify(reference), "--project", report.project, "--json"], { env, cwd: report.project });
  assert.equal(resolved.exitCode, 0, resolved.stderr.toString());
  const resolution = JSON.parse(resolved.stdout.toString());
  assert.equal(resolution.source_match, "changed");
  assert.match(readFileSync(resolution.local_path, "utf8"), /Updated documentation/);
  await page.evaluate(() => window.detachNoteFiles());
  await page.evaluate(() => window.fx_viewer.present({ action: "reveal_note", note_id: "note:root:long_document" }, { includeNotes: true }));
  assert.match(await details.innerText(), /Long-document ending retained in full/);
  await page.screenshot({ path: resolve(root, "output/playwright/workflow-notes-long-document.png") });
  // Reveal via the public opt-in action so a card under the details panel stays accessible.
  await page.evaluate(() => window.fx_viewer.present({ action: "reveal_note", note_id: "note:root:literal" }, { includeNotes: true }));
  assert.match(await details.innerText(), /\*\*This is plain text\.\*\*/);
  assert.match(await details.innerText(), /\$\{\{ inputs.prompt \}\}/);
  await page.evaluate(() => window.fx_viewer.present({ action: "reveal_note", note_id: "note:root:unavailable" }, { includeNotes: true }));
  assert.match(await details.innerText(), /Note unavailable/);
  await page.evaluate(() => window.fx_viewer.present({ action: "clear_selection" }));
  mkdirSync(resolve(root, "output/playwright"), { recursive: true });
  await page.evaluate(() => window.fx_viewer.present({ action: "reveal_note", note_id: "note:root:overview" }, { includeNotes: true }));
  await details.waitFor();
  await page.screenshot({ path: resolve(root, "output/playwright/workflow-notes.png") });
  await page.reload();
  await page.waitForSelector('.fx-canvas-note[data-note-id="note:root:overview"]');
  await nextPaint();
  assert.deepEqual(await geometry(), originalGeometry, "Reload measures the same captured note sizes before layout");
  // Exercise the maximum allowed identifier without changing the source fixture or saved run.
  const longName = "w".repeat(64);
  await page.route(`${origin}/api/snapshot`, async route => {
    const response = await route.fetch(), snapshot = await response.json();
    const note = snapshot.view.notes.find(note => note.name === "renamed_target"), previous = note.id;
    note.name = longName; note.id = `note:root:${longName}`;
    snapshot.layout.notes[note.id] = snapshot.layout.notes[previous];
    delete snapshot.layout.notes[previous];
    await route.fulfill({ response, json: snapshot });
  });
  await page.reload();
  const namedCard = page.locator(`.fx-canvas-note[data-note-id="note:root:${longName}"]`);
  await namedCard.waitFor();
  const headerFits = await namedCard.evaluate(card => card.querySelector(".fx-canvas-note-title").getBBox().width <= card.querySelector(":scope > rect").width.baseVal.value - 32);
  assert.equal(headerFits, true, "A maximum-length name must stay inside its note card");
  assert.equal(await namedCard.getAttribute("aria-label"), `Read note ${longName}`);
  await namedCard.click();
  assert.match(await details.innerText(), new RegExp(longName), "Full identifiers remain available in details");
  assert.deepEqual(external, [], "Notes must not fetch external resources");
  assert.deepEqual(errors, []);
  console.log(`ok   real notes: content-sized cards (short ${literalSize.height}, ordinary ${overviewSize.height}, long ${longSize.height}), bounded long documents, shared group tracks, stable reload/selection, Markdown/literal rendering, immutable snapshots, SDK opt-in, source opening, no external requests; $0`);
} finally {
  await browser?.close();
  server?.kill();
  if (server) await server.exited;
  rmSync(directory, { recursive: true, force: true });
}
