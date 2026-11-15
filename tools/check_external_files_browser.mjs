#!/usr/bin/env bun
/** Native+browser acceptance for external file activation. No providers, credentials or author
 * code execution. The host bridge resolves a real local path, reads it natively and presents
 * its bytes in a host-owned tab surface; the FX viewer receives no filesystem path. */
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { externalFilesFixture } from "../fixtures/viewer/external-files.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const engine = process.env.GRIDA_FX_BIN ?? resolve(root, "target/debug/grida-fx");
if (!await Bun.file(engine).exists()) throw new Error("Build the native engine first: cargo build --bin grida-fx");
const environment = { ...process.env, GRIDA_FX_NETWORK: "off", GRIDA_FX_DISABLE_DOTENV: "1" };
for (const name of ["GG_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY", "FAL_KEY", "TRIPO_API_KEY", "ELEVENLABS_API_KEY"]) delete environment[name];
const directory = mkdtempSync(resolve(tmpdir(), "fx-external-files-"));
const fixture = externalFilesFixture(directory);
const command = (args, target = fixture.run) => {
  const result = Bun.spawnSync([engine, "files", ...args.slice(0, 1), target, ...args.slice(1), "--json"], { env: environment, cwd: fixture.project });
  assert.equal(result.exitCode, 0, result.stderr.toString() || result.stdout.toString());
  return JSON.parse(result.stdout.toString());
};
const clientBuild = await Bun.build({ entrypoints: [resolve(root, "js/fx/src/viewer/client.ts")], target: "browser", format: "esm" });
assert.equal(clientBuild.success, true, String(clientBuild.logs));
const clientScript = await clientBuild.outputs[0].text();
const hostServer = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch(request) {
  if (new URL(request.url).pathname === "/client.js") return new Response(clientScript, { headers: { "Content-Type": "text/javascript" } });
  return new Response(`<!doctype html><html><head><title>FX file host fixture</title><style>
    body{font:14px system-ui;margin:32px;color:#27272a}header{display:flex;align-items:center;gap:16px}
    button{padding:8px 12px}nav{display:flex;gap:8px;margin:24px 0}section{padding:24px;border:1px solid #ddd;border-radius:8px}
    img{width:384px;max-width:100%}pre{white-space:pre-wrap}p{color:#71717a}
  </style></head><body><header><h1>Host-owned file tabs</h1><button id="attach">Connect viewer</button></header>
    <p>The viewer supplies a portable reference. This host resolves and reads the local file.</p><nav id="tabs"></nav><main id="document"></main></body></html>`, { headers: { "Content-Type": "text/html" } });
} });
const serve = (target, kind = "run") => {
  const process = Bun.spawn([engine, "view", kind === "run" ? "--run" : "--plan", target, "--port", "0"], { env: environment, cwd: fixture.project, stdout: "pipe", stderr: "pipe" });
  const url = (async () => {
    const reader = process.stdout.getReader(); let text = "";
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) throw new Error("The fixture viewer stopped before reporting readiness.");
        text += new TextDecoder().decode(value);
        const address = /^http:\/\/127\.0\.0\.1:\d+\//m.exec(text)?.[0];
        if (address) return address;
      }
    } finally { reader.releaseLock(); }
  })();
  return { process, url };
};
const runServer = serve(fixture.run), planServer = serve(fixture.builderPlan, "plan");
let browser;
try {
  const [viewerUrl, planUrl] = await Promise.all([runServer.url, planServer.url]);
  browser = await chromium.launch({ headless: true, ...(process.env.FX_CHROMIUM_EXECUTABLE ? { executablePath: process.env.FX_CHROMIUM_EXECUTABLE } : {}) });
  const context = await browser.newContext({ viewport: { width: 1100, height: 800 } });
  context.setDefaultTimeout(10_000);
  const metadataReads = [];
  context.on("request", request => { if (new URL(request.url()).pathname.endsWith("/api/files")) metadataReads.push(request.url()); });
  const errors = [];
  context.on("page", page => page.on("pageerror", error => errors.push(error.message)));
  const allowed = [new URL(viewerUrl).origin, new URL(planUrl).origin, hostServer.url.origin];
  await context.route("**/*", route => allowed.includes(new URL(route.request().url()).origin) || route.request().url().startsWith("data:") ? route.continue() : route.abort());
  const host = await context.newPage();
  await host.goto(hostServer.url.href);
  const nativeCalls = [];
  let fail = false;
  await host.exposeFunction("resolveLocalFile", async (reference) => {
    nativeCalls.push(reference);
    assert.equal(JSON.stringify(reference).includes(directory), false, "Viewer messages cannot carry a native path");
    if (fail) throw new Error("Deliberate native failure /private/not-for-viewer");
    const target = reference.subject.kind === "plan" ? fixture.builderPlan : fixture.run;
    const result = command(["resolve", "--reference", JSON.stringify(reference), "--project", fixture.project], target);
    if (!result.local_path) throw new Error("No local fixture file is available.");
    const bytes = readFileSync(result.local_path);
    return { localPath: result.local_path, name: basename(result.local_path), sourceMatch: result.source_match,
      image: reference.target.kind === "artifact" ? `data:image/png;base64,${bytes.toString("base64")}` : null,
      text: reference.target.kind === "source" ? bytes.toString("utf8") : null };
  });
  const popup = host.waitForEvent("popup");
  // A native host creates/loads its viewer surface. Direct automation navigation models that
  // load without weakening the engine's refusal of cross-site web-initiated navigation.
  await host.evaluate(() => { window.viewer = window.open("about:blank"); });
  const viewer = await popup;
  await viewer.goto(viewerUrl);
  await viewer.waitForFunction(() => { try { return window.fx_viewer.getContext().subject.kind === "run"; } catch { return false; } });
  await viewer.evaluate(() => window.fx_viewer.present({ action: "reveal_instance", instance_id: "draw#1" }));
  const link = viewer.locator(".fx-details a.preview-grid");
  await link.waitFor();
  await viewer.waitForFunction(() => document.querySelector(".fx-details img")?.complete);
  assert.equal(await viewer.locator(".fx-source-link").count(), 0, "Source labels do not become dead standalone links");
  assert.equal(metadataReads.length, 0, "Standalone viewing makes no source-inventory requests");
  const opened = viewer.waitForEvent("popup");
  await link.click();
  const assetTab = await opened;
  await assetTab.waitForURL(`**/api/artifacts/${fixture.digest}`);
  assert.equal(new URL(assetTab.url()).pathname, `/api/artifacts/${fixture.digest}`);
  await assetTab.close();
  console.log("ok   the real standalone image link opens the served browser tab");
  await viewer.evaluate(origin => { window.revokeHost = window.fx_viewer.allowHost(window.opener, origin); }, hostServer.url.origin);
  await host.evaluate(async ({ origin }) => {
    const { ViewerClient } = await import("/client.js");
    window.client = new ViewerClient({ targetWindow: window.viewer, origin });
    window.references = [];
    window.handler = async reference => {
      window.references.push(reference);
      const file = await window.resolveLocalFile(reference);
      const tab = document.createElement("button"); tab.textContent = file.name; tab.setAttribute("role", "tab");
      const show = () => {
        const panel = document.createElement("section"); panel.setAttribute("role", "tabpanel");
        const heading = document.createElement("h2"); heading.textContent = file.name; panel.append(heading);
        if (file.image) { const image = document.createElement("img"); image.src = file.image; image.alt = file.name; panel.append(image); }
        if (file.text !== null) { const pre = document.createElement("pre"); pre.textContent = file.text; panel.append(pre); }
        const note = document.createElement("p"); note.textContent = file.sourceMatch ? `Current source: ${file.sourceMatch}` : "Opened from the native local path"; panel.append(note);
        document.getElementById("document").replaceChildren(panel);
      };
      tab.onclick = show; document.getElementById("tabs").append(tab); show();
    };
    window.detach = await window.client.handleExternalFile(window.handler);
    document.getElementById("attach").textContent = "Viewer connected";
  }, { origin: new URL(viewerUrl).origin });
  const pages = context.pages().length;
  const requested = [];
  viewer.on("request", request => requested.push(new URL(request.url()).pathname));
  await link.click();
  await host.getByRole("tab", { name: "image.png", exact: true }).waitFor();
  assert.equal(context.pages().length, pages, "Host handling suppresses a browser popup");
  assert.equal(requested.filter(path => path === `/api/artifacts/${fixture.digest}`).length, 0, "The host opens local bytes without downloading from the viewer");
  assert.equal(nativeCalls.length, 1);
  assert.equal(nativeCalls[0].target.artifact.digest, fixture.digest);
  console.log("ok   the same link resolves a native local copy into the host file tab without a download or popup");
  await viewer.getByRole("button", { name: "nodes/render.py#render", exact: true }).press("Enter");
  await host.getByRole("tab", { name: "render.py", exact: true }).waitFor();
  await host.getByText("Current source: match", { exact: true }).waitFor();
  writeFileSync(resolve(fixture.project, "nodes/render.py"), "raise RuntimeError('Changed source stays editable and is never imported')\n");
  await viewer.getByRole("button", { name: "nodes/render.py#render", exact: true }).click();
  await host.getByText("Current source: changed", { exact: true }).waitFor();
  await viewer.evaluate(() => window.fx_viewer.present({ action: "clear_selection" }));
  await viewer.locator('.fx-crumbs [aria-current="page"]').click();
  await viewer.getByRole("button", { name: "workflows/child.yaml", exact: true }).click();
  await host.getByRole("tab", { name: "child.yaml", exact: true }).waitFor();
  await viewer.evaluate(() => window.fx_viewer.present({ action: "open_scope", scope_id: null }));
  await viewer.locator('.fx-crumbs [aria-current="page"]').click();
  await viewer.getByRole("button", { name: "workflows/main.yaml", exact: true }).click();
  await host.getByRole("tab", { name: "main.yaml", exact: true }).waitFor();
  console.log("ok   keyboard source activation, changed node source and root/imported YAML open without execution");
  // Register afresh after a page navigation: no stale registration or automatic reconnect.
  await host.evaluate(() => { window.client.dispose(); });
  await viewer.goto(planUrl);
  await viewer.waitForFunction(() => { try { return window.fx_viewer.getContext().subject.kind === "plan"; } catch { return false; } });
  await viewer.evaluate(origin => window.fx_viewer.allowHost(window.opener, origin), hostServer.url.origin);
  await host.evaluate(async origin => {
    const { ViewerClient } = await import("/client.js");
    window.client = new ViewerClient({ targetWindow: window.viewer, origin });
    window.detach = await window.client.handleExternalFile(window.handler);
  }, new URL(planUrl).origin);
  await viewer.locator('.fx-crumbs [aria-current="page"]').click();
  await viewer.getByRole("button", { name: "builder.py#build", exact: true }).click();
  await host.getByRole("tab", { name: "builder.py", exact: true }).waitFor();
  await host.evaluate(() => { window.client.dispose(); });
  await viewer.goto(viewerUrl);
  await viewer.waitForFunction(() => { try { return window.fx_viewer.getContext().subject.kind === "run"; } catch { return false; } });
  await viewer.evaluate(origin => window.fx_viewer.allowHost(window.opener, origin), hostServer.url.origin);
  await host.evaluate(async origin => {
    const { ViewerClient } = await import("/client.js");
    window.client = new ViewerClient({ targetWindow: window.viewer, origin });
    window.detach = await window.client.handleExternalFile(window.handler);
  }, new URL(viewerUrl).origin);
  await viewer.evaluate(() => window.fx_viewer.present({ action: "reveal_instance", instance_id: "draw#1" }));
  fail = true;
  const beforeFailure = nativeCalls.length;
  await link.click();
  await viewer.getByRole("alert").filter({ hasText: "could not handle" }).waitFor();
  assert.equal(nativeCalls.length, beforeFailure + 1);
  assert.equal(context.pages().length, pages);
  assert.equal((await viewer.locator("body").innerText()).includes("/private/not-for-viewer"), false);
  const browserFallback = viewer.waitForEvent("popup");
  await viewer.getByRole("link", { name: "Open in browser", exact: true }).click();
  await (await browserFallback).close();
  await host.evaluate(() => { window.detach(); window.client.dispose(); });
  await viewer.getByRole("button", { name: "Dismiss", exact: true }).click();
  await viewer.waitForFunction(() => document.querySelectorAll(".fx-source-link").length === 0);
  const afterDetach = viewer.waitForEvent("popup");
  await link.click();
  await (await afterDetach).close();
  assert.deepEqual(errors, []);
  const output = resolve(root, "output/playwright"); mkdirSync(output, { recursive: true });
  await host.screenshot({ path: resolve(output, "external-files-host.png"), fullPage: true });
  console.log("External files: browser link, host-owned local file tabs, source/builder/import access, changed source, failure and cleanup verified; no providers or author execution.");
} finally {
  await browser?.close();
  hostServer.stop(true);
  for (const server of [runServer, planServer]) { server.process.kill(); await server.process.exited; }
  rmSync(directory, { recursive: true, force: true });
}
