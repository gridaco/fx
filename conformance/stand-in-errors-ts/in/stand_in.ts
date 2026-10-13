// The stand-in of the stand-in-errors-ts case, stand-in-errors' stand_in.py in TypeScript: what it
// does with an image.generate call depends on the prompt, one way for each step of the workflow:
//
// - `a good picture`: an opaque PNG of the size asked for;
// - `refuse this`: throws CallRefused, a `capability_refused` error;
// - `fail this`: throws CallFailed, a `call_failed` error;
// - `decline this`: returns DECLINE;
// - `too small`: a 32x32 PNG, which the image check refuses for a 64x64 request;
// - `text instead`: text/plain bytes as `image`, which the answer's shape refuses;
// - `an extra file`: a good `image` and a `thumbnail` the capability does not return;
// - `no file at all`: an answer with no file;
// - `data as well`: a good `image`, with data where image.generate returns none.
//
// The body step's call is refused before the stand-in is asked, so it never gets here. It counts
// its calls in `count.txt` and keeps what it was asked in `asked.json`, as the other stand-in
// cases do (stand-in-image/README.md).

import { existsSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { type Answer, CallFailed, CallRefused, DECLINE, type Decline, InputFile, type StandInCall } from "@grida/fx";
import { png } from "./lib/png.ts";

const COUNT = "count.txt";
const ASKED = "asked.json";
const BLUE = [40, 90, 160];

export function answer(call: StandInCall): Answer | Decline {
  note(call);
  const prompt = call.request.prompt;
  const [width = 0, height = 0] = String(call.request.size).split("x").map(Number);
  const good = png(width, height, BLUE);
  switch (prompt) {
    case "a good picture":
      return { files: { image: good } };
    case "refuse this":
      throw new CallRefused("the stand-in refuses this prompt");
    case "fail this":
      throw new CallFailed("the stand-in failed this prompt");
    case "too small":
      return { files: { image: png(Math.floor(width / 2), Math.floor(height / 2), BLUE) } };
    case "text instead":
      return { files: { image: { bytes: new TextEncoder().encode("not a picture\n"), kind: "text/plain" } } };
    case "an extra file":
      return { files: { image: good, thumbnail: png(8, 8, BLUE) } };
    case "no file at all":
      return {};
    case "data as well":
      return { files: { image: good }, data: { seed: 7 } };
    default:
      return DECLINE;
  }
}

// ------------------------------------------------------------------------------ the notes

// Counts the call in count.txt and adds what it was asked to asked.json, sorted by each entry's
// JSON text as Python's json.dumps(entry, sort_keys=True) writes it, so the file does not depend
// on the order calls arrive in.
function note(call: StandInCall): void {
  const count = existsSync(COUNT) ? Number(readFileSync(COUNT, "utf8")) : 0;
  writeFileSync(COUNT, `${count + 1}\n`);
  const asked = existsSync(ASKED) ? (JSON.parse(readFileSync(ASKED, "utf8")) as unknown[]) : [];
  const files: { [digest: string]: unknown } = {};
  for (const [digest, file] of [...call.files].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
    files[digest] = {
      name: file.name,
      kind: file.kind,
      size: file.size,
      facts: file.facts,
      path_is_file: existsSync(file.path) && statSync(file.path).isFile(),
    };
  }
  asked.push({
    capability: call.capability,
    route: { id: call.route.id, fingerprint: call.route.fingerprint },
    request: plain(call.request),
    take: [...call.takes],
    key: call.key,
    instance: { id: call.instance.id, path: call.instance.path, step: call.instance.step },
    files,
  });
  const texts = asked.map((entry) => [sortedText(entry), entry] as const);
  texts.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  writeFileSync(ASKED, `${JSON.stringify(texts.map(([, entry]) => entry), null, 1)}\n`);
}

// A request as the wire carries it: each file as {"file": digest}.
function plain(value: unknown): unknown {
  if (value instanceof InputFile) {
    return { file: value.digest };
  }
  if (Array.isArray(value)) {
    return value.map(plain);
  }
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, plain(item)]));
  }
  return value;
}

// JSON text with sorted keys and Python's separators, as json.dumps(value, sort_keys=True).
function sortedText(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(sortedText).join(", ")}]`;
  }
  if (value !== null && typeof value === "object") {
    const keys = Object.keys(value).sort();
    const members = keys.map((key) => `${JSON.stringify(key)}: ${sortedText((value as Record<string, unknown>)[key])}`);
    return `{${members.join(", ")}}`;
  }
  return JSON.stringify(value);
}
