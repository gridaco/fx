// Real engine; metadata comes only from loopback and dry runs never generate an image.
import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { join } from "node:path";
import { run } from "../src/index.js";
import { SLOW, temporaryFolder, testEngine } from "./support.js";

const engine = testEngine();

describe.skipIf(engine === null)("SDK discovery resume", () => {
  for (const named of [false, true]) {
    test(`resumes ${named ? "by name" : "an explicit folder"} after removing metadata cache`, async () => {
      const folder = temporaryFolder("discovery-resume");
      const requests: string[] = [];
      const server = createServer((request, response) => {
        requests.push(`${request.method} ${request.url}`);
        response.writeHead(200, { "content-type": "application/json", connection: "close" });
        response.end(JSON.stringify({
          id: "acme/img-v2",
          endpoints: [{
            supported_parameters: {},
            pricing: [{ billable: "output_image", unit: "image", cost_usd: "0.05" }],
          }],
        }));
      });
      try {
        await new Promise<void>((resolve, reject) => {
          server.once("error", reject);
          server.listen(0, "127.0.0.1", resolve);
        });
        const address = server.address();
        if (address === null || typeof address === "string") throw new Error("no loopback port");
        const env = {
          GRIDA_FX_BIN: engine ?? "", GRIDA_FX_DISABLE_DOTENV: "1", GRIDA_FX_NETWORK: "on",
          GG_API_KEY: undefined, OPENAI_API_KEY: undefined, FAL_KEY: undefined,
          TRIPO_API_KEY: undefined, ELEVENLABS_API_KEY: undefined,
          HTTP_PROXY: undefined, HTTPS_PROXY: undefined, ALL_PROXY: undefined,
          OPENROUTER_API_KEY: "synthetic-resume-key",
          OPENROUTER_BASE_URL: `http://127.0.0.1:${address.port}/v1`,
        };
        writeFileSync(join(folder.path, "fx.yaml"),
          "fx: project/v1\nroutes:\n  image.generate: acme/img-v2\nprovider_preferences:\n  image.generate: openrouter\n");
        writeFileSync(join(folder.path, "workflow.yaml"),
          "fx: workflow/v1\nid: discovery\ntitle: Recorded model\nsteps:\n  draw:\n    uses: fx/image.generate@1\n    with: {prompt: a lantern}\n");
        const firstSelection = named ? { name: "offline" } : { runDir: "runs/offline" };
        const first = await run("workflow.yaml", { cwd: folder.path, env, maxUsd: 1, ...firstSelection });
        expect(first.ok).toBe(false);
        expect(first.chargedUsd).toBe(0);
        const snapshot = readFileSync(join(first.runDir, "resolved-models.json"));
        rmSync(join(folder.path, ".fx/models"), { recursive: true });
        const selection = named ? { resume: "offline" } : { runDir: "runs/offline" };
        const resumed = await run("workflow.yaml", {
          cwd: folder.path, maxUsd: 1, ...selection,
          env: { ...env, GRIDA_FX_NETWORK: "off", OPENROUTER_API_KEY: undefined },
        });
        expect(resumed.runDir).toBe(first.runDir);
        expect(resumed.ok).toBe(false);
        expect(resumed.chargedUsd).toBe(0);
        expect(readFileSync(join(first.runDir, "resolved-models.json"))).toEqual(snapshot);
        const events: { event: string }[] = readFileSync(join(first.runDir, "events.jsonl"), "utf8")
          .trim().split("\n").map((line) => JSON.parse(line));
        expect(events.filter((event) => event.event === "run_started")).toHaveLength(2);
        expect(existsSync(join(folder.path, ".fx/models"))).toBe(false);
        expect(requests).toEqual(["GET /v1/images/models/acme/img-v2/endpoints"]);
      } finally {
        await new Promise<void>((resolve) => server.close(() => resolve()));
        folder.cleanup();
      }
    }, SLOW);
  }
});
