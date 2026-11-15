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
  mkdirSync(resolve(root, "output/playwright"), { recursive: true });
  await page.screenshot({ path: resolve(root, "output/playwright/workflow-notes-initial.png") });
  const overview = page.locator('.fx-canvas-note[data-note-id="note:root:overview"]');
  assert.equal(await overview.locator("h1").innerText(), "Reference to variations");
  assert.equal(await overview.locator("img").count(), 0);
  await overview.click();
  const details = page.locator(".fx-note-details");
  await details.waitFor();
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
  assert.deepEqual(external, [], "Notes must not fetch external resources");
  assert.deepEqual(errors, []);
  console.log("ok   real notes: Markdown/literal rendering, immutable snapshots, SDK opt-in, source opening, no external requests; $0");
} finally {
  await browser?.close();
  server?.kill();
  if (server) await server.exited;
  rmSync(directory, { recursive: true, force: true });
}
