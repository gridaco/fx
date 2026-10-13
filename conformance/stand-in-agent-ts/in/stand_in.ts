// The stand-in of the stand-in-agent-ts case, stand-in-agent's stand_in.py in TypeScript.
//
// - agent.turn, for the task `Measure the lantern, then submit its height.`: the first turn calls
//   the tool `measure`; the next one, which sees the tool's answer as the last message, submits
//   the height it read there. For the task `Garble the reply.`: data that is not
//   `{"text", "tool_calls"}` (its `text` is a number), which the engine's check of agent.turn
//   refuses.
// - structured.generate: `Estimate the height of the lantern.` gets a value that meets the
//   schema; `Answer with the wrong type.` one whose `height` is text, which the schema refuses.
//
// It counts its calls in `count.txt` and keeps what it was asked in `asked.json`, in its working
// directory, which is the engine's: the project the case runs in. `asked.json` holds no path,
// only whether each file the engine handed over can be read where its ref says.

import { existsSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { Answer, DECLINE, type Decline, InputFile, type StandInCall } from "@grida/fx";

const COUNT = "count.txt";
const ASKED = "asked.json";

interface Message {
  readonly role: string;
  readonly content: string;
}

export function answer(call: StandInCall): Answer | Decline {
  note(call);
  if (call.capability === "agent.turn") {
    return turn(call);
  }
  if (call.capability === "structured.generate") {
    if (call.request.prompt === "Answer with the wrong type.") {
      return Answer.json({ height: "three", unit: "hands" });
    }
    return Answer.json({ height: 3, unit: "hands" });
  }
  return DECLINE;
}

function turn(call: StandInCall): Answer {
  const messages = call.request.messages as unknown as readonly Message[];
  if (messages[0]?.content === "Garble the reply.") {
    return { data: { text: 7, tool_calls: [] } };
  }
  const last = messages[messages.length - 1];
  if (last?.role === "tool") {
    const measured = JSON.parse(last.content) as { height: number; unit: string };
    return Answer.submit({ height: measured.height, unit: measured.unit });
  }
  return Answer.turn("I measure it first.", [
    { id: "call_measure", name: "measure", arguments: { thing: "lantern" } },
  ]);
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
