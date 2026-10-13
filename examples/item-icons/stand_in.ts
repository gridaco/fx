// Answers each icon offline, so the workflow runs without a key and at $0: a flat token on a
// transparent square of the size asked, its colour picked by the prompt, so items tell apart.
//
//   grida fx run workflows/icons.ts:build --stand-in stand_in.ts#answer

import { crc32, deflateSync } from "node:zlib";
import { type Answer, DECLINE, type Decline, type StandInCall } from "@grida/fx";

const COLORS = [
  [214, 140, 69],
  [86, 132, 201],
  [143, 99, 196],
  [92, 160, 110],
  [201, 88, 96],
  [118, 124, 135],
] as const;

export function answer(call: StandInCall): Answer | Decline {
  if (call.capability !== "image.generate") {
    return DECLINE;
  }
  const [width = 1024, height = 1024] = String(call.request.size).split("x").map(Number);
  const color = COLORS[crc32(String(call.request.prompt)) % COLORS.length] ?? COLORS[0];
  return { files: { image: token(width, height, color) } };
}

// A PNG (8-bit RGBA) of a disc of `color` filling most of a transparent picture.
function token(width: number, height: number, color: readonly number[]): Buffer {
  const radius = Math.min(width, height) * 0.4;
  const rows = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      if (Math.hypot(x + 0.5 - width / 2, y + 0.5 - height / 2) <= radius) {
        rows.set([...color, 255], y * (width * 4 + 1) + 1 + x * 4);
      }
    }
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 6, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(rows)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function chunk(type: string, data: Buffer): Buffer {
  const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const framed = Buffer.alloc(body.length + 8);
  framed.writeUInt32BE(data.length, 0);
  body.copy(framed, 4);
  framed.writeUInt32BE(crc32(body), body.length + 4);
  return framed;
}
