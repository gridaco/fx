import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

export const dynamic = "force-static";

/** The repository's llms.txt at /fx/llms.txt: one hand-kept file, exported as it is at build time. */
export async function GET(): Promise<Response> {
  const text = await readFile(resolve(process.cwd(), "..", "llms.txt"), "utf8");
  return new Response(text, { headers: { "content-type": "text/plain; charset=utf-8" } });
}
