// The stand-in of the stand-in-image-ts case, stand-in-image's stand_in.py in TypeScript: it
// answers every image.generate call with an opaque PNG of the size asked for, made here, and
// declines anything else.
//
// It counts its calls in `count.txt` and keeps what it was asked in `asked.json`, both in its
// working directory, which is the engine's: the project the case runs in. `asked.json` holds no
// path, only whether each file the engine handed over can be read where its ref says.

import { existsSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { crc32 } from "node:zlib";
import { type Answer, DECLINE, type Decline, InputFile, type StandInCall } from "@grida/fx";

const COUNT = "count.txt";
const ASKED = "asked.json";

export function answer(call: StandInCall): Answer | Decline {
  note(call);
  if (call.capability !== "image.generate") {
    return DECLINE;
  }
  const [width, height] = String(call.request.size).split("x").map(Number);
  return { files: { image: png(width ?? 0, height ?? 0, [40, 90, 160]) } };
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

// ------------------------------------------------------------------------------- pictures

// A PNG of one colour: RGB for three channels (opaque, no alpha channel), RGBA for four.
//
// The image data is zlib with stored (uncompressed) deflate blocks, written here rather than by
// zlib's deflate, so the bytes, and every digest made from them, are the same whatever zlib the
// Node that runs the stand-in was built with: stand-in-image's bytes.
function png(width: number, height: number, pixel: readonly number[]): Uint8Array {
  const colorType = pixel.length === 3 ? 2 : 6;
  const row = Buffer.concat([Buffer.from([0]), ...Array.from({ length: width }, () => Buffer.from(pixel))]);
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, colorType, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", storedZlib(Buffer.concat(Array.from({ length: height }, () => row)))),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function chunk(tag: string, data: Buffer): Buffer {
  const body = Buffer.concat([Buffer.from(tag, "latin1"), data]);
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const check = Buffer.alloc(4);
  check.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, check]);
}

// A zlib stream (RFC 1950) of stored deflate blocks (RFC 1951 section 3.2.4).
function storedZlib(data: Buffer): Buffer {
  const parts: Buffer[] = [Buffer.from([0x78, 0x01])];
  const count = Math.max(1, Math.ceil(data.length / 0xffff));
  for (let number = 1; number <= count; number++) {
    const block = data.subarray((number - 1) * 0xffff, number * 0xffff);
    const head = Buffer.alloc(5);
    head[0] = number === count ? 1 : 0;
    head.writeUInt16LE(block.length, 1);
    head.writeUInt16LE(block.length ^ 0xffff, 3);
    parts.push(head, block);
  }
  const sum = Buffer.alloc(4);
  sum.writeUInt32BE(adler32(data));
  parts.push(sum);
  return Buffer.concat(parts);
}

function adler32(data: Buffer): number {
  let a = 1;
  let b = 0;
  for (const byte of data) {
    a = (a + byte) % 65521;
    b = (b + a) % 65521;
  }
  return ((b << 16) | a) >>> 0;
}
