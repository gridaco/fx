// Pictures as RGBA pixels, and their PNG bytes: just what the badge nodes need, written with
// Node's own zlib so the project has no dependency but @grida/fx.

import { crc32, deflateSync, inflateSync } from "node:zlib";

/** A picture: `width * height` pixels, four bytes each (red, green, blue, alpha), row by row. */
export interface Picture {
  readonly width: number;
  readonly height: number;
  readonly data: Uint8Array;
}

/** A transparent picture. */
export function blank(width: number, height: number): Picture {
  return { width, height, data: new Uint8Array(width * height * 4) };
}

const SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

/** The PNG of a picture: 8-bit RGBA, every row unfiltered. */
export function encode(picture: Picture): Buffer {
  const { width, height, data } = picture;
  const rows = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) {
    rows.set(data.subarray(y * width * 4, (y + 1) * width * 4), y * (width * 4 + 1) + 1);
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 6, 0, 0, 0], 8); // 8 bits per channel, RGBA, not interlaced
  return Buffer.concat([
    SIGNATURE,
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

/** The picture in a PNG of 8-bit RGB or RGBA, not interlaced: what `encode` writes, and what most
 * tools do. Anything else throws. */
export function decode(bytes: Uint8Array): Picture {
  const png = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (!png.subarray(0, 8).equals(SIGNATURE)) {
    throw new Error("not a PNG");
  }
  let width = 0;
  let height = 0;
  let channels = 0;
  const compressed: Buffer[] = [];
  for (let at = 8; at + 8 <= png.length; ) {
    const length = png.readUInt32BE(at);
    const type = png.toString("latin1", at + 4, at + 8);
    const data = png.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      const [depth, color, , , interlace] = data.subarray(8, 13);
      channels = color === 6 ? 4 : color === 2 ? 3 : 0;
      if (depth !== 8 || channels === 0 || interlace !== 0) {
        throw new Error("only 8-bit RGB or RGBA PNGs, not interlaced, are read here");
      }
    } else if (type === "IDAT") {
      compressed.push(data);
    } else if (type === "IEND") {
      break;
    }
    at += length + 12;
  }
  const rows = inflateSync(Buffer.concat(compressed));
  const stride = width * channels;
  const pixels = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const row = rows.subarray(y * (stride + 1), (y + 1) * (stride + 1));
    unfilter(row[0] ?? 0, row.subarray(1), pixels, y, stride, channels);
  }
  const picture = blank(width, height);
  for (let pixel = 0; pixel < width * height; pixel++) {
    picture.data.set(pixels.subarray(pixel * channels, pixel * channels + 3), pixel * 4);
    picture.data[pixel * 4 + 3] = channels === 4 ? (pixels[pixel * channels + 3] ?? 0) : 255;
  }
  return picture;
}

// Undoes one row's filter (PNG specification, section 9) into row `y` of `out`.
function unfilter(
  filter: number,
  row: Uint8Array,
  out: Uint8Array,
  y: number,
  stride: number,
  step: number,
): void {
  const start = y * stride;
  for (let x = 0; x < stride; x++) {
    const left = x >= step ? (out[start + x - step] ?? 0) : 0;
    const up = y > 0 ? (out[start - stride + x] ?? 0) : 0;
    const corner = y > 0 && x >= step ? (out[start - stride + x - step] ?? 0) : 0;
    const predicted =
      filter === 1 ? left
      : filter === 2 ? up
      : filter === 3 ? (left + up) >> 1
      : filter === 4 ? paeth(left, up, corner)
      : 0;
    out[start + x] = ((row[x] ?? 0) + predicted) & 0xff;
  }
}

function paeth(left: number, up: number, corner: number): number {
  const estimate = left + up - corner;
  const toLeft = Math.abs(estimate - left);
  const toUp = Math.abs(estimate - up);
  const toCorner = Math.abs(estimate - corner);
  return toLeft <= toUp && toLeft <= toCorner ? left : toUp <= toCorner ? up : corner;
}
