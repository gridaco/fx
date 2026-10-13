// A PNG of one colour, written byte by byte: RGB for three channels (opaque, no alpha channel),
// RGBA for four. The image data is a zlib stream of stored (uncompressed) deflate blocks, with
// the Adler-32 computed here and each chunk's CRC-32 from node:zlib, so the bytes, and every
// digest made from them, are the same whatever zlib the Node that runs it was built with: those
// of stand-in-image's stand_in.py for the same size and colour.

import { crc32 } from "node:zlib";

export function png(width: number, height: number, pixel: readonly number[]): Uint8Array {
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
