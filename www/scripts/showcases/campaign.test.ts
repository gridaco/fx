import { afterEach, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, readdir, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { verifyCampaign } from "./campaign";

const temporary: string[] = [];
const hash = (content: string) => createHash("sha256").update(content).digest("hex");

afterEach(async () => {
  for (const path of temporary.splice(0)) await rm(path, { recursive: true, force: true });
});

async function fixture() {
  const root = await mkdtemp(join(await realpath(tmpdir()), "fx-showcase-approval-"));
  temporary.push(root);
  const campaign = join(root, "campaign-test");
  await mkdir(join(campaign, "publish", "example"), { recursive: true });
  const content = "<!doctype html><title>Example</title>";
  await writeFile(join(campaign, "publish", "example", "index.html"), content);
  const bundle = { showcase: "example", files: 1, bytes: Buffer.byteLength(content), tree_sha256: hash(`index.html:${hash(content)}`) };
  const inventory = { bundles: [bundle], bytes: bundle.bytes, publication_authorized: false };
  const inventoryText = JSON.stringify(inventory);
  const approval = { status: "approved", inventory: "publish-inventory.json", inventory_sha256: hash(inventoryText), bundles: [{ showcase: bundle.showcase, tree_sha256: bundle.tree_sha256 }] };
  await writeFile(join(campaign, "publish-inventory.json"), inventoryText);
  await writeFile(join(campaign, "review-approval.json"), JSON.stringify(approval));
  await writeFile(join(campaign, "private-run.json"), "Never copy this");
  return { root, campaign, inventory, approval };
}

test("verifies only the approved public tree without creating staging or publication files", async () => {
  const { root, campaign, inventory } = await fixture();
  const before = await readdir(campaign);
  expect(await verifyCampaign(campaign)).toEqual(inventory);
  expect(await verifyCampaign(campaign)).toEqual(inventory);
  expect(await readdir(campaign)).toEqual(before);
  expect(await readdir(root)).toEqual(["campaign-test"]);
  expect(await readFile(join(campaign, "private-run.json"), "utf8")).toBe("Never copy this");
});

test("rejects altered approval inventory", async () => {
  const { campaign, approval } = await fixture();
  await writeFile(join(campaign, "publish-inventory.json"), "{}\n");
  await expect(verifyCampaign(campaign)).rejects.toThrow("human approval");
  approval.inventory_sha256 = hash("{}\n");
  await writeFile(join(campaign, "review-approval.json"), JSON.stringify(approval));
  await expect(verifyCampaign(campaign)).rejects.toThrow("Invalid showcase inventory");
});

test("requires exactly the approved bundle set and inventoried publish roots", async () => {
  const { campaign, approval } = await fixture();
  approval.bundles.push({ showcase: "unlisted", tree_sha256: hash("unlisted") });
  await writeFile(join(campaign, "review-approval.json"), JSON.stringify(approval));
  await expect(verifyCampaign(campaign)).rejects.toThrow("approved bundle set");
  approval.bundles.pop();
  await writeFile(join(campaign, "review-approval.json"), JSON.stringify(approval));
  await writeFile(join(campaign, "publish", "private.json"), "Do not copy");
  await expect(verifyCampaign(campaign)).rejects.toThrow("exactly the inventoried bundles");
});

test("tree digests follow Python Path component order", async () => {
  const { campaign, inventory, approval } = await fixture();
  const folder = join(campaign, "publish", "example");
  await mkdir(join(folder, "asset"));
  await writeFile(join(folder, "asset", "image.png"), "image");
  await writeFile(join(folder, "asset.json"), "metadata");
  const index = await readFile(join(folder, "index.html"), "utf8");
  const bundle = inventory.bundles[0];
  bundle.files = 3;
  bundle.bytes += Buffer.byteLength("imagemetadata");
  bundle.tree_sha256 = hash([`asset/image.png:${hash("image")}`, `asset.json:${hash("metadata")}`, `index.html:${hash(index)}`].join("\n"));
  inventory.bytes = bundle.bytes;
  const inventoryText = JSON.stringify(inventory);
  approval.inventory_sha256 = hash(inventoryText);
  approval.bundles[0].tree_sha256 = bundle.tree_sha256;
  await writeFile(join(campaign, "publish-inventory.json"), inventoryText);
  await writeFile(join(campaign, "review-approval.json"), JSON.stringify(approval));
  expect((await verifyCampaign(campaign)).bundles).toEqual(inventory.bundles);
});

test("rechecks approved source bytes and never rewrites them", async () => {
  const { campaign } = await fixture();
  await verifyCampaign(campaign);
  const source = join(campaign, "publish", "example", "index.html");
  await writeFile(source, "Changed source");
  await expect(verifyCampaign(campaign)).rejects.toThrow("Bundle verification failed");
  expect(await readFile(source, "utf8")).toBe("Changed source");
});

test("rejects bundle name traversal in the approved inventory", async () => {
  const { campaign, inventory, approval } = await fixture();
  inventory.bundles[0].showcase = "../escape";
  approval.bundles[0].showcase = "../escape";
  const text = JSON.stringify(inventory);
  approval.inventory_sha256 = hash(text);
  await writeFile(join(campaign, "publish-inventory.json"), text);
  await writeFile(join(campaign, "review-approval.json"), JSON.stringify(approval));
  await expect(verifyCampaign(campaign)).rejects.toThrow("Invalid approved bundle set");
});

test("rejects symlink source files", async () => {
  const { root, campaign } = await fixture();
  const path = join(campaign, "publish", "example", "index.html");
  await rm(path);
  await writeFile(join(root, "outside.html"), "External content");
  await symlink(join(root, "outside.html"), path);
  await expect(verifyCampaign(campaign)).rejects.toThrow("symlinks");
});

test("rejects symlink campaign ancestors", async () => {
  const { root, campaign } = await fixture();
  const alias = join(root, "alias");
  await symlink(root, alias);
  await expect(verifyCampaign(join(alias, "campaign-test"))).rejects.toThrow("real directories");
  expect(await verifyCampaign(campaign)).toHaveProperty("bundles");
});
