// The stand-in of the stand-in-calls-ts case. It answers image.generate with a blue PNG and
// image.edit with an orange one, each of the size asked for, and structured.generate with a
// report of the first picture in its context: the size the engine measured, read from the
// file's facts. It declines anything else.
//
// It counts its calls in `count.txt` and keeps what it was asked in `asked.json`, as the other
// stand-in cases do (stand-in-image/README.md).

import { existsSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { Answer, DECLINE, type Decline, InputFile, type StandInCall } from "@grida/fx";
import { png } from "./lib/png.ts";

const COUNT = "count.txt";
const ASKED = "asked.json";

export function answer(call: StandInCall): Answer | Decline {
  note(call);
  if (call.capability === "image.generate" || call.capability === "image.edit") {
    const [width, height] = String(call.request.size).split("x").map(Number);
    const pixel = call.capability === "image.generate" ? [40, 90, 160] : [200, 120, 40];
    return { files: { image: png(width ?? 0, height ?? 0, pixel) } };
  }
  if (call.capability === "structured.generate") {
    const [first] = call.request.context as readonly InputFile[];
    return Answer.json({ subject: "lantern", width: first?.facts.width ?? 0, height: first?.facts.height ?? 0 });
  }
  return DECLINE;
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
