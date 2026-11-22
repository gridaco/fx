#!/usr/bin/env bun
/** Exercise both race orders and disconnected clients through the bundled run viewer and CLI. */
import assert from "node:assert/strict";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { respond } from "../js/fx/dist/index.js";

const root = fileURLToPath(new URL("../", import.meta.url));
const engine = process.env.GRIDA_FX_BIN ?? resolve(root, "target/debug/grida-fx");
const directory = mkdtempSync(resolve(tmpdir(), "fx-review-browser-"));
const project = resolve(directory, "project");
cpSync(resolve(root, "fixtures/waiting"), project, { recursive: true });
const env = {
  ...process.env, GRIDA_FX_NETWORK: "off", GRIDA_FX_DISABLE_DOTENV: "1",
  GRIDA_FX_PYTHON: process.env.GRIDA_FX_PYTHON ?? resolve(root, "python/.venv/bin/python"),
};
const cli = args => Bun.spawnSync([engine, ...args], { cwd: project, env });
const json = args => {
  const result = cli(args);
  assert.equal(result.exitCode, 0, result.stderr.toString() || result.stdout.toString());
  return JSON.parse(result.stdout.toString());
};
const spawn = args => Bun.spawn([engine, ...args], { cwd: project, env, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
async function eventually(check, description) {
  const end = Date.now() + 20_000;
  while (Date.now() < end) {
    const result = await check();
    if (result) return result;
    await Bun.sleep(100);
  }
  throw new Error(`Timed out waiting for ${description}`);
}

function reviewCard(page, step) {
  const card = page.locator(`[data-node-id="instance:${step}#1"]`);
  return {
    card,
    review: card.getByRole("button", { name: /^Review(?: |$)/ }),
    receipt: card.getByRole("button", { name: /^View answer(?: |$)/ }),
  };
}

const reviewPanel = page => page.getByRole("complementary", { name: "Workflow requests" });
async function assertPanelClosed(page) {
  await reviewPanel(page).waitFor({ state: "hidden" });
  assert.equal(await page.getByRole("button", { name: "Submit answer" }).count(), 0);
}

// Call with zero after the runner parks or finishes, when the ordinary run icon is idle.
async function waitForReviewTab(page, count) {
  const title = count === 0 ? "Run" : count === 1 ? "Awaiting answer" : `Awaiting ${count} answers`;
  const href = new URL(count ? "favicon-awaiting.svg" : "favicon.svg", page.url()).href;
  await page.waitForFunction(({ title, href }) => {
    const icon = document.querySelector('link[rel="icon"]');
    return document.title.startsWith(`${title} · `) && icon?.href === href;
  }, { title, href });
  return href;
}

// The service accepts fixed ports. Release an OS-selected loopback port immediately before binding.
const probe = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response() });
const port = probe.port;
await probe.stop(true);
const origin = `http://127.0.0.1:${port}`;
const service = spawn(["start", "--port", String(port)]);
let browser, runner, responder;
try {
  await eventually(async () => (await fetch(`${origin}/api/catalog`).catch(() => null))?.ok, "project readiness");
  browser = await chromium.launch({ headless: true, ...(process.env.FX_CHROMIUM_EXECUTABLE ? { executablePath: process.env.FX_CHROMIUM_EXECUTABLE } : {}) });
  const context = await browser.newContext({ viewport: { width: 1280, height: 840 }, reducedMotion: "reduce" });
  await context.route("**/*", route => new URL(route.request().url()).origin === origin ? route.continue() : route.abort());
  const home = await context.newPage();
  await home.goto(origin);
  await home.getByRole("heading", { name: "Workflows", exact: true }).waitFor();
  for (const name of ["viewer-first", "agent-first", "agent-disconnected"]) {
    const browserWins = name !== "agent-first";
    const attached = name === "viewer-first";
    const agentOnline = name !== "agent-disconnected";
    const target = `review-variants/${name}`;
    runner = spawn(["run", "./workflow.yaml", "--name", name, "--no-view", ...(attached ? ["--wait-for-answers"] : [])]);
    // The agent's CLI observes the exact named run before the foreground run call returns.
    const pending = await eventually(() => {
      const result = cli(["inspect", target, "--requests", "--json"]);
      if (result.exitCode !== 0) return;
      const { requests } = JSON.parse(result.stdout.toString());
      if (requests.length === 2) return requests;
    }, "the variant and revision requests");
    const opened = pending.find(item => item.step === "choose#1");
    const revision = pending.find(item => item.step === "revision#1");
    assert.ok(opened && revision);
    assert.equal(Object.hasOwn(revision, "message"), false);
    if (!attached) assert.equal(await runner.exited, 1, "detached invocation returns incomplete");
    const access = json(["inspect", target, "--requests", "--register", "--json"]);
    assert.deepEqual(access.requests, pending);
    assert.equal(Object.hasOwn(json(["inspect", target, "--requests", "--json"]), "viewer_url"), false);
    assert.equal(json(["inspect", target, "--requests", "--viewer-url", "--json"]).viewer_url, access.viewer_url);
    const url = new URL(access.viewer_url);
    assert.equal(url.origin, origin);
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    if (attached) await page.emulateMedia({ reducedMotion: "no-preference" });
    await page.goto(url.href);
    const panel = reviewPanel(page);
    const runHeader = page.getByRole("navigation", { name: "Workflow location" });
    const homeRun = home.locator(`a.fx-project-entry[href="${url.pathname}"]`);
    const choose = reviewCard(page, "choose");
    try {
      await choose.review.waitFor({ timeout: 10_000 });
      await assertPanelClosed(page);
      await waitForReviewTab(page, 2);
      await reviewCard(page, "revision").review.click();
      await panel.getByRole("heading", { name: "Build revision", exact: true }).waitFor();
      const supplied = await respond(target, revision.id, "rev-42", { cwd: project, env: { ...env, GRIDA_FX_BIN: engine } });
      assert.equal(supplied.state.channel, "sdk");
      const revisionReceipt = panel.getByRole("status").filter({ hasText: "Answered via sdk" });
      await revisionReceipt.getByText("rev-42", { exact: true }).waitFor();
      await panel.getByRole("button", { name: "Close review" }).click();
      const icon = await waitForReviewTab(page, 1);
      await runHeader.getByText("Awaiting answer", { exact: true }).waitFor();
      await homeRun.getByText("Awaiting answer", { exact: true }).waitFor();
      if (attached) {
        const snapshot = await page.request.get(new URL("api/snapshot", url).href);
        assert.equal(snapshot.headers()["x-fx-run-active"], "1", "the service should observe the attached runner");
        const asset = await page.request.get(icon);
        assert.equal(asset.status(), 200);
        assert.match(asset.headers()["content-type"], /^image\/svg\+xml\b/);
        assert.match(await asset.text(), /<svg[\s>]/);
        // The live green icon normally alternates every 650 ms. Amber must remain steady.
        const changes = await page.evaluate(async () => {
          const link = document.querySelector('link[rel="icon"]');
          const urls = new Set([link.href]);
          const observer = new MutationObserver(() => urls.add(link.href));
          observer.observe(link, { attributes: true, attributeFilter: ["href"] });
          await new Promise(resolve => setTimeout(resolve, 1450));
          observer.disconnect();
          return [...urls];
        });
        assert.deepEqual(changes, [icon]);
      }
      if (attached && process.env.FX_WAITING_CARD_SCREENSHOT)
        await page.screenshot({ path: process.env.FX_WAITING_CARD_SCREENSHOT });
      if (attached) {
        await page.getByRole("navigation", { name: "Workflow location" }).locator("[aria-current=page]").click();
        await page.getByRole("complementary", { name: "Details", exact: true }).waitFor();
        await choose.review.press("Enter");
      } else {
        await choose.review.click();
      }
      await panel.getByRole("heading", { name: "Choose a variant" }).waitFor({ timeout: 10_000 });
      if (attached) {
        assert.equal(await page.getByRole("complementary", { name: "Details", exact: true }).count(), 0);
        const pane = page.getByRole("main", { name: "Workflow canvas" });
        const toolbar = page.getByRole("navigation", { name: "Workflow location" });
        for (const width of [1280, 550]) {
          await page.setViewportSize({ width, height: 840 });
          const frame = await pane.boundingBox();
          const review = await panel.boundingBox();
          assert.equal(frame.x + frame.width - review.x - review.width, 12);
          if (width === 550) {
            const bar = await toolbar.boundingBox();
            assert.ok(review.y >= bar.y + bar.height, "the narrow panel should sit beneath the toolbar");
          }
        }
        await page.setViewportSize({ width: 1280, height: 840 });
      }
      const cardCountdown = choose.card.getByRole("timer");
      const panelCountdown = panel.getByRole("timer");
      await cardCountdown.waitFor();
      await panelCountdown.waitFor();
      assert.equal(await panelCountdown.getAttribute("datetime"), opened.deadline);
      if (attached) {
        const initial = await panelCountdown.textContent();
        assert.match(initial, /^\d+:\d{2} left$/);
        await eventually(async () => (await panelCountdown.textContent()) !== initial, "the panel countdown to tick");
        const initialCard = await cardCountdown.textContent();
        await eventually(async () => (await cardCountdown.textContent()) !== initialCard, "the canvas countdown to tick");
      }
    } catch (cause) {
      const view = await (await fetch(new URL("api/run", url))).json();
      throw new Error(JSON.stringify({ run: view.state, nodes: view.nodes?.map(n => [n.id, n.uses, n.state]),
        page: await page.locator("body").innerText(), errors }), { cause });
    }
    await page.waitForFunction(() => {
      const images = [...document.querySelectorAll(".fx-review-choice img")];
      return images.length === 3 && images.every(image => image.complete && image.naturalWidth > 0);
    });
    if (attached) {
      await panel.getByRole("button", { name: "Submit answer" }).press("Enter");
      await panel.getByRole("alert").filter({ hasText: "Choose a candidate." }).waitFor();
      const first = panel.getByRole("radio", { name: "a", exact: true });
      assert.ok(await first.evaluate(radio => radio === document.activeElement));
      assert.equal(await first.getAttribute("aria-invalid"), "true");
      assert.ok((await first.locator("..").boundingBox()).height >= 44, "the candidate label should be a large pointer target");
      await first.press("ArrowRight");
      assert.ok(await panel.getByRole("radio", { name: "b", exact: true }).isChecked());
      assert.equal(await first.getAttribute("aria-invalid"), null);
    } else {
      await panel.getByRole("radio", { name: "b", exact: true }).check();
    }
    if (attached) {
      await panel.getByRole("button", { name: "Close review" }).click();
      await assertPanelClosed(page);
      await page.getByRole("complementary", { name: "Details", exact: true }).waitFor();
      await choose.review.focus();
      await choose.review.press("Enter");
      const choice = panel.getByRole("radio", { name: "b", exact: true });
      await choice.waitFor();
      assert.ok(await choice.isChecked(), "closing a pending review discarded the selected variant");
    }
    if (attached && process.env.FX_WAITING_SCREENSHOT) await page.screenshot({ path: process.env.FX_WAITING_SCREENSHOT });
    // For the two races, hold one writer in flight. The disconnected case leaves both the
    // runner and CLI offline until the browser has recorded its answer and reloaded.
    responder = agentOnline ? spawn(["respond", target, opened.id, "--json", "-"]) : null;
    const sendAgent = () => { responder.stdin.write('{"variant":"c"}'); responder.stdin.end(); return responder.exited; };
    const apiPath = `**/api/requests/${opened.id}/respond`;
    let agentExit;
    if (!browserWins) await page.route(apiPath, async route => {
      agentExit = await sendAgent();
      await route.continue();
    }, { times: 1 });
    const receipt = page.waitForResponse(response => response.url().endsWith(`/requests/${opened.id}/respond`));
    await panel.getByRole("button", { name: "Submit answer" }).click();
    const response = await receipt;
    assert.equal(response.status(), browserWins ? 200 : 409);
    if (browserWins && agentOnline) agentExit = await sendAgent();
    const body = await response.json();
    const winner = (browserWins ? body : body.request).state;
    const key = browserWins ? "b" : "c";
    assert.equal(winner.answer.variant, key);
    assert.equal(winner.channel, browserWins ? "viewer" : "cli");
    const receiptPanel = panel.getByRole("status").filter({ hasText: `Answered via ${winner.channel}` });
    const verifyWinner = async () => {
      await receiptPanel.waitFor();
      assert.equal(await panel.getByRole("timer").count(), 0);
      assert.equal(await choose.card.getByRole("timer").count(), 0);
      const choice = receiptPanel.getByRole("figure", { name: `Variant: ${key}`, exact: true });
      await choice.getByText(key, { exact: true }).waitFor();
      const preview = choice.getByRole("img", { name: opened.choices[key].file.name, exact: true });
      await preview.waitFor();
      const previewUrl = new URL(await preview.getAttribute("src"), url);
      assert.equal(previewUrl.href, new URL(`api/artifacts/${opened.choices[key].file.digest}`, url).href);
      assert.equal(await receiptPanel.locator("pre,code").count(), 0);
      assert.equal(await receiptPanel.locator("time").getAttribute("datetime"), winner.at);
      assert.equal(await panel.getByRole("button", { name: "Submit answer" }).count(), 0);
    };
    await verifyWinner();
    if (!attached) await waitForReviewTab(page, 0);
    await panel.getByRole("button", { name: "Close review" }).click();
    await assertPanelClosed(page);
    await page.reload();
    await choose.receipt.waitFor();
    await assertPanelClosed(page);
    await choose.receipt.click();
    await verifyWinner();
    if (attached && process.env.FX_WAITING_RECEIPT_SCREENSHOT)
      await page.screenshot({ path: process.env.FX_WAITING_RECEIPT_SCREENSHOT });
    await panel.getByRole("button", { name: "Close review" }).click();
    await assertPanelClosed(page);
    if (agentOnline) {
      assert.equal(agentExit === 0, !browserWins);
      const agentReceipt = JSON.parse(await new Response(responder.stdout).text());
      assert.deepEqual(agentReceipt.request.state, winner);
    }
    const observed = json(["inspect", target, "--requests", "--json"]).requests.find(item => item.id === opened.id);
    assert.deepEqual(observed.state, winner);
    if (!attached) {
      await waitForReviewTab(page, 0);
      // The detached answer updates both surfaces without a reload or a new invocation.
      await runHeader.getByText("unfinished", { exact: true }).waitFor();
      await homeRun.getByText("unfinished", { exact: true }).waitFor();
      runner = spawn(["run", "./workflow.yaml", "--resume", name, "--no-view"]);
    }
    assert.equal(await runner.exited, 0, await new Response(runner.stderr).text());
    const { card } = choose;
    await card.getByText("succeeded · Selected", { exact: true }).waitFor();
    await waitForReviewTab(page, 0);
    await runHeader.getByText("succeeded", { exact: true }).waitFor();
    await homeRun.getByText("succeeded", { exact: true }).waitFor();
    assert.match(await card.getAttribute("aria-label"), /succeeded, Selected/);
    if (attached && process.env.FX_WAITING_OUTCOME_SCREENSHOT)
      await page.screenshot({ path: process.env.FX_WAITING_OUTCOME_SCREENSHOT });
    const view = await (await fetch(new URL("api/run", url))).json();
    assert.equal(view.outputs.image.file.digest, opened.choices[key].file.digest);
    const { run } = json(["inspect", target, "--json"]);
    const manifest = JSON.parse(readFileSync(resolve(project, run.folder, "outputs/manifest.json"), "utf8"));
    assert.deepEqual(manifest, { revision: "rev-42", image_digest: opened.choices[key].file.digest });
    assert.deepEqual(errors, []);
    await page.close();
    console.log(`ok   ${name}: CLI, viewer and downstream agree on variant ${key}`);
  }
  writeFileSync(resolve(project, "feedback.yaml"), `fx: workflow/v1
id: feedback
title: Give review feedback
steps:
  review:
    uses: fx/await@1
    with:
      message: How did this variant do?
      schema:
        type: object
        required: [approved, note, scores, metadata, flags]
        additionalProperties: false
        properties:
          approved: { title: Approve the variant, type: boolean }
          note: { title: Review note, type: string, minLength: 3, description: Describe what should change. }
          scores: { type: array, items: { type: integer } }
          metadata: { type: object, additionalProperties: false }
          flags: { type: array, items: { type: boolean } }
          alternate: { type: object, required: [reason], properties: { reason: { type: string } } }
          tags: { type: array, items: { type: string } }
          counters: { type: object, patternProperties: { '^count_': { type: integer } }, additionalProperties: { type: boolean } }
  delivery:
    uses: fx/await@1
    with: { message: Which delivery format?, schema: { type: string } }
  confirmation:
    needs: [delivery]
    uses: fx/await@1
    with: { message: Confirm the delivery format, schema: { type: boolean }, timeout_seconds: 8, on_timeout: continue }
`);
  runner = spawn(["run", "./feedback.yaml", "--name", "structured", "--no-view"]);
  assert.equal(await runner.exited, 1, "park with both reviews open");
  const structured = json(["inspect", "feedback/structured", "--requests", "--register", "--json"]);
  assert.equal(structured.requests.length, 2);
  const page = await context.newPage();
  await page.goto(structured.viewer_url);
  const panel = reviewPanel(page);
  const feedback = reviewCard(page, "review");
  const delivery = reviewCard(page, "delivery");
  const confirmation = reviewCard(page, "confirmation");
  const feedbackHeading = panel.getByRole("heading", { name: "How did this variant do?", exact: true });
  const deliveryHeading = panel.getByRole("heading", { name: "Which delivery format?", exact: true });
  await feedback.review.waitFor();
  await delivery.review.waitFor();
  await assertPanelClosed(page);
  await waitForReviewTab(page, 2);
  await feedback.review.click();
  await feedbackHeading.waitFor();
  assert.equal(await feedback.card.getByRole("timer").count(), 0);
  const awaiting = feedback.card.getByText("Awaiting answer", { exact: true });
  await awaiting.waitFor();
  assert.equal(await awaiting.evaluate(text => text.previousElementSibling?.classList.contains("fx-state-icon") ?? false), false);
  await panel.getByText("Awaiting answer", { exact: true }).waitFor();
  assert.equal(await panel.getByRole("timer").count(), 0);
  assert.equal(await deliveryHeading.count(), 0);
  const submit = panel.getByRole("button", { name: "Submit answer" });
  await panel.getByRole("group", { name: "Approve the variant" }).getByRole("radio", { name: "No" }).check();
  const note = panel.getByRole("textbox", { name: "Review note", exact: true });
  assert.equal(await note.getAttribute("aria-required"), "true");
  await submit.press("Enter");
  const missingNote = panel.getByRole("alert").filter({ hasText: "Enter Review note." });
  await missingNote.waitFor();
  assert.ok(await note.evaluate(input => input === document.activeElement));
  assert.deepEqual(await note.evaluate(input => input.getAttribute("aria-describedby").split(" ")
    .map(id => document.getElementById(id).textContent)), ["Describe what should change.", "Enter Review note."]);
  await note.fill("x");
  await submit.press("Enter");
  const refusal = panel.getByRole("alert").filter({ hasText: "Check the fields in your answer and try again." });
  await refusal.waitFor();
  await eventually(() => refusal.evaluate(element => element === document.activeElement), "focus on the engine validation error");
  await note.fill("Try a softer palette");
  assert.equal(await note.getAttribute("aria-invalid"), null);
  const addItem = panel.getByRole("group", { name: "Scores", exact: true }).getByRole("button", { name: "Add item" });
  await panel.getByRole("button", { name: "Include Tags", exact: true }).click();
  const counters = panel.getByRole("group", { name: "Counters", exact: true });
  await counters.getByRole("button", { name: "Include Counters", exact: true }).click();
  await counters.getByRole("button", { name: "Add field", exact: true }).click();
  await counters.getByRole("textbox", { name: "Field name", exact: true }).fill("count_prints");
  await counters.getByRole("combobox", { name: "Value type", exact: true }).selectOption("number");
  await counters.getByRole("textbox", { name: "Value", exact: true }).fill("4");
  await addItem.press("Enter");
  const item1 = panel.getByRole("textbox", { name: "Item 1", exact: true });
  await eventually(() => item1.evaluate(input => input === document.activeElement), "focus on the new first item");
  await item1.fill("0");
  await addItem.press("Enter");
  const item2 = panel.getByRole("textbox", { name: "Item 2", exact: true });
  await eventually(() => item2.evaluate(input => input === document.activeElement), "focus on the new second item");
  await item2.fill("7");
  await addItem.press("Enter");
  const item3 = panel.getByRole("textbox", { name: "Item 3", exact: true });
  await eventually(() => item3.evaluate(input => input === document.activeElement), "focus on the new last item");
  await panel.getByRole("button", { name: "Remove item 3" }).press("Enter");
  await item3.waitFor({ state: "detached" });
  await eventually(() => addItem.evaluate(button => button === document.activeElement), "focus after removing the last item");
  assert.equal(await panel.locator("pre,code,textarea").count(), 0);
  await page.keyboard.press("Escape");
  await assertPanelClosed(page);
  assert.ok(await feedback.review.evaluate(button => button === document.activeElement), "Escape should return focus to the originating card");
  await delivery.review.click();
  await deliveryHeading.waitFor();
  assert.equal(await feedbackHeading.count(), 0);
  await page.getByRole("navigation", { name: "Workflow location" }).locator("[aria-current=page]").click();
  await assertPanelClosed(page);
  const details = page.getByRole("complementary", { name: "Details", exact: true });
  await details.getByRole("button", { name: "Close details" }).click();
  await feedback.review.click();
  await feedbackHeading.waitFor();
  assert.equal(await panel.getByRole("textbox", { name: "Review note", exact: true }).inputValue(), "Try a softer palette");
  assert.equal(await panel.getByRole("textbox", { name: "Item 2", exact: true }).inputValue(), "7");
  const deliveryRequest = structured.requests.find(request => request.step === "delivery#1");
  assert.ok(deliveryRequest);
  json(["respond", "feedback/structured", deliveryRequest.id, "--json", '"png"']);
  await delivery.receipt.waitFor();
  // One request remains until a runner opens the follow-up. The tab must count the detached answer.
  await waitForReviewTab(page, 1);
  runner = spawn(["run", "./feedback.yaml", "--resume", "structured", "--no-view", "--wait-for-answers"]);
  await confirmation.review.waitFor();
  await feedbackHeading.waitFor();
  assert.equal(await deliveryHeading.count(), 0);
  assert.equal(await panel.getByRole("heading", { name: "Confirm the delivery format", exact: true }).count(), 0);
  assert.equal(await panel.getByRole("textbox", { name: "Review note", exact: true }).inputValue(), "Try a softer palette");
  await panel.getByRole("button", { name: "Submit answer" }).click();
  const receipt = panel.getByRole("status").filter({ hasText: "Answered via viewer" });
  await receipt.getByText("Try a softer palette", { exact: true }).waitFor();
  await eventually(() => receipt.evaluate(element => element === document.activeElement), "focus on the answer receipt");
  assert.equal(await receipt.locator("pre,code").count(), 0);
  const recorded = () => json(["inspect", "feedback/structured", "--requests", "--json"]).requests;
  assert.deepEqual(recorded().find(request => request.step === "review#1").state.answer,
    { approved: false, note: "Try a softer palette", scores: [0, 7], metadata: {}, flags: [], tags: [], counters: { count_prints: 4 } });
  assert.equal(await runner.exited, 0, await new Response(runner.stderr).text());
  await feedback.card.getByText("succeeded · Answered", { exact: true }).waitFor();
  await confirmation.card.getByText("succeeded · Expired", { exact: true }).waitFor();
  assert.equal(await confirmation.card.getByRole("timer").count(), 0);
  await waitForReviewTab(page, 0);
  const expired = recorded().find(request => request.step === "confirmation#1").state;
  assert.equal(expired.status, "expired");
  await receipt.getByText("Try a softer palette", { exact: true }).waitFor();
  await panel.getByRole("button", { name: "Close review" }).click();
  await confirmation.card.getByRole("button", { name: /^View outcome(?: |$)/ }).click();
  await panel.getByRole("heading", { name: "Confirm the delivery format", exact: true }).waitFor();
  const expiredReceipt = panel.getByRole("status").filter({ hasText: "Expired" });
  await expiredReceipt.waitFor();
  assert.equal(await panel.getByRole("timer").count(), 0);
  assert.equal(await expiredReceipt.locator("time").getAttribute("datetime"), expired.at);
  assert.equal(await panel.getByRole("button", { name: "Submit answer" }).count(), 0);
  console.log("ok   simultaneous reviews: keyboard, draft, and feedback receipt stay with the chosen card through another answer and expiry");
  await page.close();

  const showcase = "review-showcase/typed-forms";
  runner = spawn(["run", "./showcase.yaml", "--name", "typed-forms", "--max-usd", "0", "--no-view"]);
  assert.equal(await runner.exited, 1, await new Response(runner.stderr).text());
  const printRequest = requests => requests.find(request => request.step === "print_settings#1");
  const forms = json(["inspect", showcase, "--requests", "--register", "--json"]);
  assert.equal(printRequest(forms.requests)?.state.status, "pending");
  assert.equal(Object.hasOwn(printRequest(forms.requests), "choices"), false);
  const formPage = await context.newPage();
  await formPage.goto(forms.viewer_url);
  const formPanel = reviewPanel(formPage);
  const print = reviewCard(formPage, "print_settings");
  await print.review.waitFor();
  await print.review.press("Enter");
  await formPanel.getByRole("heading", { name: "How should the print edition be produced?", exact: true }).waitFor();
  const finish = formPanel.getByRole("combobox", { name: "Paper finish", exact: true });
  assert.equal(await finish.inputValue(), "");
  await finish.selectOption({ label: "satin" });
  if (process.env.FX_WAITING_ENUM_SCREENSHOT)
    await formPage.screenshot({ path: process.env.FX_WAITING_ENUM_SCREENSHOT });
  await formPanel.getByRole("textbox", { name: "Print copies", exact: true }).fill("12");
  await formPanel.getByRole("textbox", { name: "Scale factor", exact: true }).fill("0.95");
  const margins = formPanel.getByRole("group", { name: "Margins", exact: true });
  await margins.getByRole("textbox", { name: "Top margin in mm", exact: true }).fill("3.5");
  const profile = formPanel.getByRole("group", { name: "Color profile override", exact: true });
  await profile.getByRole("checkbox", { name: "No value", exact: true }).check();
  const labels = formPanel.getByRole("group", { name: "Production labels", exact: true });
  await labels.getByRole("button", { name: "Include Production labels", exact: true }).click();
  await labels.getByRole("button", { name: "Add field", exact: true }).click();
  await labels.getByRole("textbox", { name: "Field name", exact: true }).fill("batch");
  await labels.getByRole("textbox", { name: "Value", exact: true }).fill("proof");
  await formPanel.getByRole("button", { name: "Submit answer" }).click();
  const formReceipt = formPanel.getByRole("status").filter({ hasText: "Answered via viewer" });
  await formReceipt.getByText("proof", { exact: true }).waitFor();
  assert.equal(await formReceipt.locator("pre,code").count(), 0);
  assert.deepEqual(printRequest(json(["inspect", showcase, "--requests", "--json"]).requests)?.state.answer, {
    finish: "satin", copies: 12, scale: 0.95, margins: { top_mm: 3.5 }, profile_override: null, labels: { batch: "proof" },
  });
  await formPage.close();
  console.log("ok   showcase print settings: schema enum, typed fields, nested values and explicit null share an answer");
} finally {
  await browser?.close();
  for (const child of [responder, runner, service]) if (child && child.exitCode === null) child.kill();
  await Promise.all([responder, runner, service].filter(Boolean).map(child => child.exited));
  rmSync(directory, { recursive: true, force: true });
}
