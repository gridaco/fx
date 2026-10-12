import { join } from "node:path";
import { deliveryBase } from "../../src/lib/showcases/catalog";
import { wwwRoot } from "./io";
import { verifyShowcases, writeBuildBinding } from "./verify";
import { checkImagePolicy } from "../image-policy";
import { checkSitemap } from "../sitemap";

await checkImagePolicy();
const verified = await verifyShowcases({ requireRecordings: true });
const base = deliveryBase();
const child = Bun.spawn([process.execPath, "--no-env-file", "run", "build:next"], {
  cwd: wwwRoot, env: { ...process.env, NEXT_TELEMETRY_DISABLED: "1", FX_SHOWCASE_ASSET_BASE: base },
  stdin: "inherit", stdout: "inherit", stderr: "inherit",
});
const exit = await child.exited;
if (exit) process.exit(exit);
await checkSitemap(join(wwwRoot, "out"));
const binding = await writeBuildBinding(base, { output: join(wwwRoot, "out"), expectedLock: verified.lock_sha256 });
console.log(`Bound website output ${binding.output_sha256} to content ${binding.lock_sha256} at ${binding.delivery_base}.`);
