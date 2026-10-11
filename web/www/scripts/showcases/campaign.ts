import { createHash } from "node:crypto";
import { lstat, readFile, readdir } from "node:fs/promises";
import { join, resolve } from "node:path";
import { confinedFile } from "./io";

type Bundle = { showcase: string; files: number; bytes: number; tree_sha256: string };
export type CampaignInventory = { bundles: Bundle[]; bytes: number };
type Approval = { status: string; inventory: string; inventory_sha256: string; bundles: Pick<Bundle, "showcase" | "tree_sha256">[] };
type Tree = { paths: string[]; files: number; bytes: number; tree_sha256: string };

const sha256 = (data: string | Uint8Array) => createHash("sha256").update(data).digest("hex");
const slug = (value: unknown): value is string => typeof value === "string" && /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(value);
const digest = (value: unknown) => typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
const count = (value: unknown) => Number.isSafeInteger(value) && (value as number) >= 0;

async function plainDirectory(path: string) {
  const info = await lstat(path);
  if (!info.isDirectory() || info.isSymbolicLink()) throw new Error("Showcase directories must be real directories, without symlinks.");
}

async function plainFile(path: string) {
  const info = await lstat(path);
  if (!info.isFile() || info.isSymbolicLink()) throw new Error("Showcase files must be regular files, without symlinks.");
  return readFile(path);
}

async function tree(root: string): Promise<Tree> {
  await plainDirectory(root);
  const paths: string[] = [];
  async function walk(relative: string) {
    for (const entry of await readdir(join(root, relative), { withFileTypes: true })) {
      if (/[\\\r\n\0]/.test(entry.name)) throw new Error("Showcase filenames must be portable relative paths.");
      const path = relative ? `${relative}/${entry.name}` : entry.name;
      const info = await lstat(join(root, path));
      if (info.isSymbolicLink()) throw new Error("Showcase bundles must not contain symlinks.");
      if (info.isDirectory()) await walk(path);
      else if (info.isFile()) paths.push(path);
      else throw new Error("Showcase bundles must contain only directories and regular files.");
    }
  }
  await walk("");
  // Python's sorted(Path) compares path components, then Unicode values within each component.
  paths.sort((a, b) => {
    const left = a.split("/");
    const right = b.split("/");
    for (let i = 0; i < Math.min(left.length, right.length); i++) {
      const order = Buffer.compare(Buffer.from(left[i]), Buffer.from(right[i]));
      if (order) return order;
    }
    return left.length - right.length;
  });
  let bytes = 0;
  const records: string[] = [];
  for (const path of paths) {
    const content = await plainFile(join(root, path));
    bytes += content.byteLength;
    records.push(`${path}:${sha256(content)}`);
  }
  return { paths, files: paths.length, bytes, tree_sha256: sha256(records.join("\n")) };
}

function validateDocuments(inventory: CampaignInventory, approval: Approval, inventoryBytes: Uint8Array) {
  if (approval.status !== "approved" || approval.inventory !== "publish-inventory.json" || approval.inventory_sha256 !== sha256(inventoryBytes)) {
    throw new Error("A human approval bound to this exact publish inventory is required.");
  }
  if (!Array.isArray(inventory.bundles) || !inventory.bundles.length || !count(inventory.bytes) || !Array.isArray(approval.bundles)) {
    throw new Error("Invalid showcase inventory or approval.");
  }
  const approved = new Map<string, string>();
  for (const bundle of approval.bundles) {
    if (!slug(bundle.showcase) || !digest(bundle.tree_sha256) || approved.has(bundle.showcase)) throw new Error("Invalid approved bundle set.");
    approved.set(bundle.showcase, bundle.tree_sha256);
  }
  const seen = new Set<string>();
  let bytes = 0;
  for (const bundle of inventory.bundles) {
    if (!slug(bundle.showcase) || !digest(bundle.tree_sha256) || !count(bundle.files) || !count(bundle.bytes) || seen.has(bundle.showcase)) {
      throw new Error("Invalid bundle in publish inventory.");
    }
    if (approved.get(bundle.showcase) !== bundle.tree_sha256) throw new Error("The inventory and approved bundle set must match exactly.");
    seen.add(bundle.showcase);
    bytes += bundle.bytes;
  }
  if (seen.size !== approved.size || bytes !== inventory.bytes) throw new Error("The inventory totals or approved bundle set do not match.");
}

/** Verify the approved input tree without copying bundles or publishing their old HTML. */
export async function verifyCampaign(campaignDirectory: string): Promise<CampaignInventory> {
  const campaign = resolve(campaignDirectory);
  const inventoryBytes = await confinedFile(campaign, "publish-inventory.json");
  const inventory: CampaignInventory = JSON.parse(inventoryBytes.toString("utf8"));
  const approval: Approval = JSON.parse((await confinedFile(campaign, "review-approval.json")).toString("utf8"));
  validateDocuments(inventory, approval, inventoryBytes);
  const source = join(campaign, "publish");
  await plainDirectory(source);
  const actual = (await readdir(source)).sort();
  const expected = inventory.bundles.map(bundle => bundle.showcase).sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error("The publish directory must contain exactly the inventoried bundles.");
  for (const bundle of inventory.bundles) {
    const measured = await tree(join(source, bundle.showcase));
    if (measured.files !== bundle.files || measured.bytes !== bundle.bytes || measured.tree_sha256 !== bundle.tree_sha256) {
      throw new Error(`Bundle verification failed: ${bundle.showcase}.`);
    }
  }
  return inventory;
}
