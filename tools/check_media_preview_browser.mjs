#!/usr/bin/env bun
/** Browser acceptance for the real, provider-free viewer-media-previews recording.
 * bun --no-env-file tools/check_media_preview_browser.mjs --url http://127.0.0.1:PORT/
 * bun --no-env-file tools/check_media_preview_browser.mjs --run PATH --command target/debug/grida-fx
 * FX_CHROMIUM_EXECUTABLE optionally selects an installed Chromium, as in the other browser checks.
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = fileURLToPath(new URL("../", import.meta.url));
const usage = "Use --url URL or --run PATH [--command ENGINE]";
if (process.argv.includes("--help")) { console.log(usage); process.exit(0); }
const options = {};
for (let index = 2; index < process.argv.length; index += 2) {
  const name = process.argv[index], value = process.argv[index + 1];
  if (!["--url", "--run", "--command"].includes(name) || !value) throw new Error(usage);
  options[name.slice(2)] = value;
}
assert(Boolean(options.url) !== Boolean(options.run), "Choose exactly one of --url and --run");
assert(!options.command || options.run, "--command requires --run");

let viewer;
async function hostRun(path) {
  const environment = { ...process.env, GRIDA_FX_DISABLE_DOTENV: "1", GRIDA_FX_NETWORK: "off" };
  for (const key of ["GG_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY", "FAL_KEY", "TRIPO_API_KEY", "ELEVENLABS_API_KEY"]) delete environment[key];
  const command = resolve(options.command ?? process.env.GRIDA_FX_BIN ?? resolve(root, "target/debug/grida-fx"));
  let output = "", failure;
  viewer = spawn(command, ["view", "--run", resolve(path), "--port", "0", "--no-open"], { cwd: dirname(resolve(path)), env: environment, stdio: ["ignore", "pipe", "pipe"] });
  viewer.on("error", error => { failure = error; });
  for (const stream of [viewer.stdout, viewer.stderr]) stream.on("data", data => { output = (output + data).slice(-8000); });
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    if (failure) throw failure;
    const url = /^http:\/\/127\.0\.0\.1:\d+\/\s*$/m.exec(output)?.[0].trim();
    if (url) return url;
    assert(viewer.exitCode === null, `Viewer exited before listening: ${output.trim()}`);
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  throw new Error("Viewer did not report a loopback address within 15 seconds");
}
async function stopViewer() {
  if (!viewer?.pid || viewer.exitCode !== null || viewer.signalCode !== null) return;
  const stopped = new Promise(resolve => viewer.once("exit", resolve));
  viewer.kill("SIGTERM");
  const timer = setTimeout(() => viewer.kill("SIGKILL"), 5000);
  try { await stopped; } finally { clearTimeout(timer); }
}

let browser, page;
function colorVariety(bytes) {
  const colors = new Set();
  for (let index = 0; index < bytes.length; index += 4) colors.add(bytes.slice(index, index + 3).join(","));
  return colors.size;
}
try {
  const base = new URL(options.url ?? await hostRun(options.run));
  assert(base.protocol === "http:" && ["127.0.0.1", "localhost"].includes(base.hostname), "The media fixture must be served on loopback");
  if (!base.pathname.endsWith("/")) base.pathname += "/";
  const response = await fetch(new URL("api/view", base));
  assert(response.ok, "The fixture viewer must expose its recorded view");
  const run = await response.json();
  assert.equal(run.kind, "fx-viewer-run-v1");
  assert.equal(run.workflow.id, "viewer-media-previews");
  assert.equal(run.state, "succeeded"); assert.equal(run.charged_usd, 0);
  const names = ["image", "video", "unsupported_video", "custom_poster", "unsupported_poster", "static_model", "animated_model"];
  const nodes = new Map(names.map(name => {
    const node = run.nodes.find(node => node.path === name);
    assert(node, `Missing ${name} fixture card`); assert.equal(node.state, "succeeded");
    return [name, node];
  }));
  assert.equal(run.nodes.length, names.length, "This fixture is a focused seven-card media comparison");
  const artifacts = new Map(run.artifacts.map(file => [file.digest, file]));
  const output = (name, port) => {
    const file = artifacts.get(nodes.get(name).outputs[port].file.digest);
    assert(file?.available && file.url, `${name}/${port} must retain its recorded source`);
    return file;
  };

  browser = await chromium.launch({ headless: true, ...(process.env.FX_CHROMIUM_EXECUTABLE ? { executablePath: process.env.FX_CHROMIUM_EXECUTABLE } : {}) });
  const context = await browser.newContext({ viewport: { width: 1440, height: 1100 }, reducedMotion: "no-preference" });
  context.setDefaultTimeout(20_000); context.setDefaultNavigationTimeout(15_000);
  const errors = [], writes = [], outbound = [];
  await context.route("**/*", route => {
    const request = route.request(), url = new URL(request.url());
    if (!["GET", "HEAD"].includes(request.method())) writes.push(request.method());
    if (url.origin === base.origin || ["blob:", "data:"].includes(url.protocol)) return route.continue();
    outbound.push(url.href); return route.abort();
  });
  page = await context.newPage(); page.on("pageerror", error => errors.push(error.message));
  await page.goto(base.href);
  await page.waitForFunction(() => { try { return window.fx_viewer.getContext().subject.kind === "run"; } catch { return false; } });
  const card = name => page.locator(`[data-node-id=${JSON.stringify(`instance:${nodes.get(name).id}`)}]`);
  const video = name => card(name).locator(".fx-canvas-video");
  const videoPlayer = page.locator(".fx-canvas-player");
  const modelPlayer = page.locator(".fx-canvas-model-player");
  const clear = () => page.evaluate(() => window.fx_viewer.present({ action: "clear_selection" }));
  const reveal = async name => {
    await page.evaluate(id => window.fx_viewer.present({ action: "reveal_instance", instance_id: id }), nodes.get(name).id);
    await clear();
    await page.locator(".fx-details").waitFor({ state: "detached" });
  };
  const selection = () => page.evaluate(() => window.fx_viewer.getContext().selection);
  const samples = async locator => locator.evaluate(async node => {
    const image = new Image(); image.src = node.getAttribute("href"); await image.decode();
    const canvas = document.createElement("canvas"); canvas.width = canvas.height = 16;
    const ctx = canvas.getContext("2d"); ctx.drawImage(image, 0, 0, 16, 16);
    return Array.from(ctx.getImageData(0, 0, 16, 16).data);
  });
  const posterClick = async name => {
    await card(name).locator(".fx-canvas-model-open").click({ position: { x: 10, y: 10 } });
    await page.locator(".fx-canvas-model-player.is-shown").waitFor();
    await page.waitForFunction(() => document.querySelector(".fx-canvas-model-player model-viewer")?.loaded);
    assert.equal(await selection(), null); assert.equal(await page.locator(".fx-details").count(), 0);
  };
  const modelStill = async name => {
    await reveal(name);
    await card(name).locator(".fx-canvas-model.has-still").waitFor();
    const pixels = await samples(card(name).locator(".fx-canvas-model-still"));
    assert(colorVariety(pixels) > 1, "The model thumbnail must contain rendered geometry");
  };

  await reveal("image");
  const image = card("image").locator(".fx-canvas-node-image");
  assert.equal(await image.getAttribute("href"), output("image", "image").url);
  assert(colorVariety(await samples(image)) > 1);
  console.log("ok   actual recorded PNG renders on the image card");

  await reveal("video");
  await page.waitForFunction(id => document.querySelector(`[data-node-id="instance:${id}"] .fx-canvas-video-still`)?.getAttribute("href")?.startsWith("blob:"), nodes.get("video").id);
  await video("video").hover({ position: { x: 12, y: 12 } });
  await page.locator(".fx-canvas-player.is-shown").waitFor();
  await page.waitForFunction(() => { const video = document.querySelector(".fx-canvas-player video"); return video.currentTime > .1 && !video.paused && video.muted && video.loop; });
  await page.mouse.move(1420, 20);
  await videoPlayer.waitFor({ state: "hidden" });
  await card("video").locator(".fx-canvas-play").click();
  await page.locator(".fx-canvas-player.is-shown").waitFor();
  await videoPlayer.getByRole("button", { name: "Pause", exact: true }).click();
  await videoPlayer.getByRole("button", { name: "Full screen", exact: true }).click();
  await page.waitForFunction(() => document.fullscreenElement === document.querySelector(".fx-canvas-player"));
  await videoPlayer.getByRole("button", { name: "Exit full screen", exact: true }).click();
  await page.waitForFunction(() => document.fullscreenElement === null);
  await page.keyboard.press("Escape"); await videoPlayer.waitFor({ state: "hidden" });
  assert.equal(await selection(), null);
  console.log("ok   native VP9 has a captured first frame, muted hover playback, manual controls and fullscreen");

  for (const name of ["unsupported_video", "unsupported_poster"]) {
    await reveal(name);
    await card(name).locator(".fx-canvas-video.is-unavailable").waitFor();
    assert.equal(await card(name).locator(".fx-canvas-play").getAttribute("aria-disabled"), "true");
    if (name === "unsupported_poster") {
      await card(name).locator(".fx-canvas-video.has-poster").waitFor();
      assert.equal(await card(name).locator(".fx-canvas-video-poster").getAttribute("href"), output(name, "poster").url);
    }
    const label = name === "unsupported_video" ? "Browser preview unavailable" : "Browser playback unavailable";
    assert((await card(name).locator(".fx-canvas-image-error").textContent()).includes(label));
    await video(name).hover({ position: { x: 12, y: 12 } });
    await new Promise(resolve => setTimeout(resolve, 450));
    assert.equal(await videoPlayer.isVisible(), false, "Unsupported files must not autoplay or replace their fallback");
    await page.evaluate(id => window.fx_viewer.present({ action: "reveal_instance", instance_id: id }), nodes.get(name).id);
    await page.locator(".fx-details").getByText(`${label}. Download the original to try it in another video player.`, { exact: true }).waitFor();
    const download = page.locator(".fx-details").getByRole("link", { name: "Download original video", exact: true });
    assert.equal(await download.getAttribute("href"), output(name, "video").url);
    const downloaded = page.waitForEvent("download"); await download.click();
    await (await downloaded).cancel();
    await clear();
  }
  console.log("ok   Chromium's unsupported FFV1 keeps a stable failure, original download, and custom poster when supplied");

  await reveal("custom_poster");
  await card("custom_poster").locator(".fx-canvas-video.has-poster").waitFor();
  const custom = card("custom_poster").locator(".fx-canvas-video-poster");
  assert.equal(await custom.getAttribute("href"), output("custom_poster", "poster").url);
  await page.waitForFunction(id => document.querySelector(`[data-node-id="instance:${id}"] .fx-canvas-video-still`)?.getAttribute("href")?.startsWith("blob:"), nodes.get("custom_poster").id);
  const posterPixels = await samples(custom), framePixels = await samples(card("custom_poster").locator(".fx-canvas-video-still"));
  assert(posterPixels.some((value, index) => Math.abs(value - framePixels[index]) > 100), "The authored poster must differ from the native video frame");
  await card("custom_poster").locator(".fx-canvas-play").click();
  await page.waitForFunction(() => { const video = document.querySelector(".fx-canvas-player video"); return !video.paused && video.currentTime > .1; });
  await page.keyboard.press("Escape"); await videoPlayer.waitFor({ state: "hidden" });
  assert.equal(await custom.getAttribute("href"), output("custom_poster", "poster").url);
  console.log("ok   authored poster remains distinct from the playable clip and returns after playback");

  for (const name of ["static_model", "animated_model"]) {
    await modelStill(name); await posterClick(name);
    const model = modelPlayer.locator("model-viewer");
    const clips = await model.evaluate(model => model.availableAnimations);
    if (name === "static_model") assert.deepEqual(clips, []);
    else {
      assert.deepEqual(clips, ["Turn", "Bob"]);
      await modelPlayer.getByLabel("Animation", { exact: true }).selectOption("Bob");
      await modelPlayer.getByRole("button", { name: "Play", exact: true }).click();
      await page.waitForFunction(() => document.querySelector(".fx-canvas-model-player model-viewer").currentTime > .1);
      await modelPlayer.getByRole("button", { name: "Pause", exact: true }).click();
      const handle = await model.elementHandle();
      await modelPlayer.getByRole("button", { name: "Full screen", exact: true }).click();
      await page.waitForFunction(() => document.fullscreenElement === document.querySelector(".fx-canvas-model-player"));
      assert(await handle.evaluate(node => node.isConnected));
      await page.keyboard.press("Escape");
      await page.waitForFunction(() => document.fullscreenElement === null);
      assert.equal(await modelPlayer.isVisible(), true);
    }
    await modelPlayer.getByRole("button", { name: "Back to poster", exact: true }).click();
    assert.equal(await modelPlayer.isVisible(), false);
  }
  console.log("ok   static and animated GLBs render their own posters, activate in place, expose applicable controls and return with Back");

  await clear(); await page.getByRole("button", { name: "Fit workflow", exact: true }).click();
  await page.mouse.move(1420, 20);
  mkdirSync(resolve(root, "output/playwright"), { recursive: true });
  await page.screenshot({ path: resolve(root, "output/playwright/media-previews.png") });
  assert.deepEqual(errors, []); assert.deepEqual(outbound, []); assert.deepEqual(writes, []);
  console.log("ok   all seven media cases share one real offline recording; no outbound requests, HTTP writes or browser exceptions");
} finally {
  if (page) await page.evaluate(() => document.fullscreenElement ? document.exitFullscreen() : undefined).catch(() => {});
  try { await browser?.close(); } finally { await stopViewer(); }
}
