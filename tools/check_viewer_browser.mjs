#!/usr/bin/env bun
/** Provider-free browser checks against the built viewer and the public client source.
 * bun --no-env-file run build:viewer
 * bun --no-env-file x playwright-core install chromium
 * bun --no-env-file run test:viewer:browser
 * FX_CHROMIUM_EXECUTABLE optionally selects an already installed Chromium.
 */
import assert from "node:assert/strict";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = fileURLToPath(new URL("../", import.meta.url));
const assets = resolve(root, "web/viewer/dist");
if (!await Bun.file(resolve(assets, "index.html")).exists()) throw new Error("Build the viewer first: bun --no-env-file run build:viewer");
const clientBuild = await Bun.build({ entrypoints: [resolve(root, "js/fx/src/viewer/client.ts")], target: "browser", format: "esm" });
assert.equal(clientBuild.success, true, String(clientBuild.logs));
const clientScript = await clientBuild.outputs[0].text();

// Synthetic recorded data: no author code, providers, credentials, or engine processes.
const instances = [1, 2, 3].map((take) => ({
  id: `prepare#${take}`, path: "sub.prepare", step: "sub.prepare", take: [1, take], uses: "./nodes.py#prepare",
  type: null, with: {}, routes: {}, state: "planned", identity: null, phase: 1, key: null,
  judges: null, judged_by: [], waiting_on: [], needs: [], reads: [], price: { low_usd: 0, high_usd: 0 },
}));
const plan = {
  kind: "fx-graph-v1", workflow: { id: "sample", title: "Sample", file: "workflows/sample.yaml" },
  instances, pending: [], problems: [], estimate: { low_usd: 0, high_usd: 0, ceiling_usd: null },
  scopes: [{ id: "scope:sub#1", parent: null, kind: "workflow", path: "sub", step: "sub", take: [1], title: "Imported workflow",
    source: null, ports: { inputs: {}, outputs: [] }, input_bindings: [], output_bindings: [], nodes: instances.map((node) => node.id), pending: [] }],
};
const run = {
  kind: "fx-viewer-run-v1", workflow: { id: "sample", title: "Sample" }, run_name: "recorded", state: "succeeded", stand_in: false,
  charged_usd: 0, estimate: null, inputs: {}, outputs: {}, artifacts: [], warnings: [], scopes: plan.scopes,
  nodes: instances.map((node) => ({
    id: node.id, path: node.path, step: node.step, take: node.take, title: `Prepare ${node.take.at(-1)}`, uses: node.uses,
    state: "succeeded", reads: [], with: Object.fromEntries(Array.from({ length: 25 }, (_, at) => [`field_${at}`, "synthetic value"])),
    outputs: {}, cache: null, error: null, duration_ms: 0,
  })),
};
const requests = [];
const hostHtml = () => new Response("<!doctype html><title>Synthetic viewer host</title>", { headers: { "Content-Type": "text/html" } });
const server = Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(request) {
  const { pathname } = new URL(request.url);
  requests.push({ method: request.method, pathname });
  if (request.method !== "GET") return new Response("Read only", { status: 405 });
  if (pathname.endsWith("/api/view")) return Response.json(pathname.includes("/plans/") ? plan : run);
  if (pathname.endsWith("/api/snapshot")) return Response.json({ kind: "fx-run-snapshot-v1", cursor: "fixture", plan, events: [], view: run });
  if (pathname.endsWith("/api/events")) return Response.json({ kind: "fx-run-event-batch-v1", cursor: "fixture", events: [], has_more: false });
  if (pathname.endsWith("/api/layout")) return new Response("No fixture report", { status: 404 });
  if (pathname === "/viewer-client.js") return new Response(clientScript, { headers: { "Content-Type": "text/javascript" } });
  if (pathname === "/host") return hostHtml();
  const asset = /^.*\/assets\/([^/]+)$/.exec(pathname);
  return new Response(Bun.file(asset ? resolve(assets, "assets", asset[1]) : resolve(assets, "index.html")));
} });
const foreign = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: hostHtml });
const origin = server.url.origin;
const foreignOrigin = foreign.url.origin;
let browser;

/** Assert actual card geometry, including animated transforms, against the unobscured canvas. */
async function visibleCard(page, id) {
  await page.waitForFunction((id) => {
    const card = [...document.querySelectorAll("[data-node-id]")].find((node) => node.getAttribute("data-node-id") === `instance:${id}`);
    const pane = document.querySelector(".fx-canvas-pane");
    if (!card || !pane) return false;
    const box = card.getBoundingClientRect(), area = pane.getBoundingClientRect();
    const panel = document.querySelector(".fx-details")?.getBoundingClientRect();
    const list = document.querySelector(".fx-steps")?.getBoundingClientRect();
    const bar = document.querySelector(".fx-bar")?.getBoundingClientRect();
    return box.width > 0 && box.height > 0
      && box.left >= (list?.right ?? area.left) + 8 && box.right <= (panel?.left ?? area.right) - 8
      && box.top >= (bar?.bottom ?? area.top) + 8 && box.bottom <= area.bottom - 8;
  }, id, { timeout: 5000 });
}
async function ready(page, kind = "run") {
  await page.waitForFunction((kind) => {
    try { return window.fx_viewer.getContext().subject.kind === kind; } catch { return false; }
  }, kind);
  await page.getByRole("group", { name: "Workflow dependency canvas", exact: true }).waitFor();
}

try {
  browser = await chromium.launch({ headless: true, ...(process.env.FX_CHROMIUM_EXECUTABLE ? { executablePath: process.env.FX_CHROMIUM_EXECUTABLE } : {}) });
  const context = await browser.newContext({ viewport: { width: 600, height: 650 }, reducedMotion: "no-preference" });
  context.setDefaultTimeout(10_000);
  context.setDefaultNavigationTimeout(10_000);
  const pageErrors = [];
  const consoleErrors = [];
  context.on("page", (page) => {
    page.on("pageerror", (error) => pageErrors.push(error.message));
    page.on("console", (message) => { if (message.type() === "error" && !message.text().includes("404")) consoleErrors.push(message.text()); });
  });
  await context.route("**/*", (route) => [origin, foreignOrigin].includes(new URL(route.request().url()).origin) ? route.continue() : route.abort());
  const host = await context.newPage();
  await host.goto(`${origin}/host`);
  const firstPopup = host.waitForEvent("popup");
  await host.evaluate(() => { window.first = window.open("/"); });
  const page = await firstPopup;
  const secondPopup = host.waitForEvent("popup");
  await host.evaluate(() => { window.second = window.open("/"); });
  const other = await secondPopup;
  await Promise.all([ready(page), ready(other)]);
  await host.evaluate(async () => {
    const { ViewerClient } = await import("/viewer-client.js");
    window.Client = ViewerClient;
    window.client = new ViewerClient({ targetWindow: window.first, origin: location.origin, timeoutMs: 1000 });
    window.updates = [];
    window.unsubscribe = await window.client.subscribe((context) => window.updates.push(context));
  });

  // New details must be committed and measured before the first reveal in a narrow pane.
  const revealed = await host.evaluate(() => window.client.present({ action: "reveal_instance", instance_id: "prepare#2" }));
  assert.equal(revealed.scope_id, "scope:sub#1");
  assert.deepEqual(revealed.selection.take, [1, 2]);
  await visibleCard(page, "prepare#2");
  assert.equal((await other.evaluate(() => window.fx_viewer.getContext())).selection, null);
  console.log("ok   narrow-pane reveal measures new details and selects the exact take independently");

  await page.getByRole("button", { name: "Steps", exact: true }).click();
  await host.evaluate(() => window.client.present({ action: "reveal_instance", instance_id: "prepare#1" }));
  assert.equal(await page.locator(".fx-steps").count(), 0);
  await visibleCard(page, "prepare#1");

  // A person's selection updates the host; an open grid remains open when a member is revealed.
  await page.getByRole("button", { name: "Close details", exact: true }).click();
  await host.waitForFunction(() => window.updates.length >= 3 && window.updates.at(-1).selection === null);
  await page.locator(".fx-deck-badge").click();
  assert.equal(await page.locator(".fx-deck-bar").isVisible(), true);
  await page.mouse.move(50, 500);
  await page.mouse.wheel(3000, 3000);
  await page.waitForFunction(() => document.querySelector('[data-node-id="instance:prepare#3"]').getBoundingClientRect().bottom < 0);
  await host.evaluate(() => window.client.present({ action: "reveal_instance", instance_id: "prepare#3" }));
  await visibleCard(page, "prepare#3");
  assert.equal(await page.locator(".fx-deck-bar").isVisible(), true);
  console.log("ok   an expanded-deck member is revealed at its grid position after panning away");

  const camera = () => page.locator(".fx-canvas > svg > g").evaluate((node) => node.style.transform);
  const before = await camera();
  await host.evaluate(() => window.client.present({ action: "clear_selection" }));
  assert.equal(await camera(), before);
  assert.equal((await page.evaluate(() => window.fx_viewer.getContext())).selection, null);
  const refusal = await host.evaluate(async () => {
    try { await window.client.present({ action: "reveal_instance", instance_id: "missing" }); } catch (error) { return error.code; }
  });
  assert.equal(refusal, "not_found");
  assert.equal(await camera(), before);
  const fitted = await host.evaluate(() => window.client.present({ action: "fit" }));
  assert.equal(fitted.selection, null);
  await host.evaluate(() => window.client.present({ action: "open_scope", scope_id: null }));
  await host.evaluate(() => { window.unsubscribe(); window.client.dispose(); });
  console.log("ok   clear selection preserves the camera; unknown targets refuse; fit and root navigation apply");

  await page.reload();
  await ready(page);
  const fresh = await host.evaluate(async () => {
    const client = new window.Client({ targetWindow: window.first, origin: location.origin });
    const context = await client.getContext(); client.dispose(); return context;
  });
  assert.equal(fresh.selection, null);
  await page.goto(`${origin}/p/${"a".repeat(64)}/plans/${"b".repeat(64)}/`);
  await ready(page, "plan");
  const scoped = await page.evaluate(() => window.fx_viewer.getContext());
  assert.equal(scoped.subject.reference.entry_id, "b".repeat(64));
  console.log("ok   reload supports a fresh client and a scoped plan exposes its recorded reference");

  const outsider = await context.newPage();
  await outsider.goto(`${foreignOrigin}/`);
  const popup = outsider.waitForEvent("popup");
  await outsider.evaluate((origin) => { window.viewer = window.open(origin); }, origin);
  const viewer = await popup;
  await ready(viewer);
  await outsider.evaluate(async ({ code, origin }) => {
    const { ViewerClient } = await import(`data:text/javascript;base64,${btoa(code)}`);
    window.client = new ViewerClient({ targetWindow: window.viewer, origin, timeoutMs: 500 });
  }, { code: clientScript, origin });
  const probe = () => outsider.evaluate(async () => {
    try { return (await window.client.getContext()).subject.kind; } catch (error) { return error.code; }
  });
  assert.equal(await probe(), "timeout");
  await viewer.evaluate((origin) => { window.revoke = window.fx_viewer.allowHost(window.opener, origin); }, foreignOrigin);
  assert.equal(await probe(), "run");
  await viewer.evaluate(() => window.revoke());
  assert.equal(await probe(), "timeout");
  await outsider.evaluate(() => window.client.dispose());
  assert.deepEqual(pageErrors, []);
  assert.deepEqual(consoleErrors, []);
  assert(requests.every((request) => request.method === "GET"), "Presentation must not write through the HTTP API");
  console.log("ok   cross-origin refusal, explicit grant and revocation; no page errors or HTTP writes");
} finally {
  try { await browser?.close(); }
  finally { server.stop(true); foreign.stop(true); }
}
