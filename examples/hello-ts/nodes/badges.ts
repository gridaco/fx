// The hello-ts example's own nodes: draw one badge, judge its background, scale it down, and lay
// badges out on one sheet. Pictures go through #lib/png.ts; nothing here needs Python.

import { node } from "@grida/fx";
import { blank, decode, encode, type Picture } from "#lib/png.ts";

const SIDE = 256; // a badge is drawn on a 256 x 256 transparent square; `shrink` scales it down
const INSET = 32; // the clear margin around the shape
const SAMPLES = 4; // 4 x 4 samples a pixel, for smooth edges

export const badge = node(
  {
    name: "badge",
    params: {
      shape: ["circle", "square", "diamond"],
      color: { type: "string", pattern: "^#[0-9a-f]{6}$" },
    },
    outputs: { image: "image/png" },
  },
  async (ctx) => {
    const middle = SIDE / 2;
    const reach = middle - INSET;
    // Whether the point (x, y) is inside the shape: tsc checks that every shape has a case.
    const inside = (x: number, y: number): boolean => {
      const dx = Math.abs(x - middle);
      const dy = Math.abs(y - middle);
      switch (ctx.params.shape) {
        case "circle":
          return dx * dx + dy * dy <= reach * reach;
        case "square":
          return dx <= reach && dy <= reach;
        case "diamond":
          return dx + dy <= reach;
      }
    };
    const color = ctx.params.color;
    const fill = [1, 3, 5].map((at) => Number.parseInt(color.slice(at, at + 2), 16));
    const picture = blank(SIDE, SIDE);
    for (let y = 0; y < SIDE; y++) {
      for (let x = 0; x < SIDE; x++) {
        let covered = 0;
        for (let sy = 0; sy < SAMPLES; sy++) {
          for (let sx = 0; sx < SAMPLES; sx++) {
            covered += Number(inside(x + (sx + 0.5) / SAMPLES, y + (sy + 0.5) / SAMPLES));
          }
        }
        if (covered > 0) {
          const alpha = Math.round((255 * covered) / SAMPLES ** 2);
          picture.data.set([...fill, alpha], (y * SIDE + x) * 4);
        }
      }
    }
    return { image: ctx.out.png(encode(picture)) };
  },
);

// A judge: a badge must have a clear background, as fx/image.check_alpha@1 judges it in the
// Python hello. A rejection fails the badge, and `small` waits for the verdict.
export const clear = node(
  { name: "clear", inputs: { image: "image" }, outputs: {}, judge: true },
  async (ctx) => {
    const { data } = decode(await ctx.inputs.image.bytes());
    let low = 255;
    let high = 0;
    for (let at = 3; at < data.length; at += 4) {
      low = Math.min(low, data[at] ?? 0);
      high = Math.max(high, data[at] ?? 0);
    }
    await ctx.fact("alpha_min", low);
    await ctx.fact("alpha_max", high);
    await ctx.fact("verdict", low === 0 ? "accept" : "reject");
    return {};
  },
);

// Scales a picture so its longest side is `side`, averaging the pixels each new one covers.
export const shrink = node(
  {
    name: "shrink",
    inputs: { image: "image" },
    params: { side: { type: "integer", minimum: 1 } },
    outputs: { image: "image/png" },
  },
  async (ctx) => {
    const source = decode(await ctx.inputs.image.bytes());
    const scale = ctx.params.side / Math.max(source.width, source.height);
    const width = Math.max(1, Math.round(source.width * scale));
    const height = Math.max(1, Math.round(source.height * scale));
    return { image: ctx.out.png(encode(resample(source, width, height))) };
  },
);

export const sheet = node(
  {
    name: "sheet",
    inputs: { badges: "image{}" },
    params: { gap: "integer" },
    outputs: { image: "image/png" },
  },
  async (ctx) => {
    const gap = ctx.params.gap;
    const pictures: Picture[] = [];
    for (const file of ctx.inputs.badges.values()) {
      pictures.push(decode(await file.bytes()));
    }
    if (pictures.length === 0) {
      throw ctx.fail("no badges to lay out");
    }
    const widths = pictures.reduce((sum, picture) => sum + picture.width, 0);
    const width = widths + gap * (pictures.length + 1);
    const height = Math.max(...pictures.map((picture) => picture.height)) + 2 * gap;
    const canvas = blank(width, height);
    let left = gap;
    for (const picture of pictures) {
      for (let y = 0; y < picture.height; y++) {
        const row = picture.data.subarray(y * picture.width * 4, (y + 1) * picture.width * 4);
        canvas.data.set(row, ((gap + y) * width + left) * 4);
      }
      left += picture.width + gap;
    }
    await ctx.fact("count", pictures.length);
    return { image: ctx.out.png(encode(canvas)) };
  },
);

// ------------------------------------------------------------------------------- resampling

// `source` at `width` x `height`: each new pixel is the area-weighted average of the pixels it
// covers, with colours weighted by their alpha so transparent pixels add no colour.
function resample(source: Picture, width: number, height: number): Picture {
  const columns = spans(source.width, width);
  const rows = spans(source.height, height);
  const out = blank(width, height);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const sum = [0, 0, 0, 0];
      let area = 0;
      for (const [sy, wy] of rows[y] ?? []) {
        for (const [sx, wx] of columns[x] ?? []) {
          const at = (sy * source.width + sx) * 4;
          const weight = wx * wy;
          const alpha = (source.data[at + 3] ?? 0) * weight;
          for (let channel = 0; channel < 3; channel++) {
            sum[channel] = (sum[channel] ?? 0) + (source.data[at + channel] ?? 0) * alpha;
          }
          sum[3] = (sum[3] ?? 0) + alpha;
          area += weight;
        }
      }
      const alpha = sum[3] ?? 0;
      if (alpha > 0) {
        const color = sum.slice(0, 3).map((value) => Math.round(value / alpha));
        out.data.set([...color, Math.round(alpha / area)], (y * width + x) * 4);
      }
    }
  }
  return out;
}

// For each of `to` new pixels along one side, the old pixels it covers, with how much of each:
// [[index, weight], …].
function spans(from: number, to: number): [number, number][][] {
  const step = from / to;
  return Array.from({ length: to }, (_, index) => {
    const start = index * step;
    const end = start + step;
    const covered: [number, number][] = [];
    for (let old = Math.floor(start); old < Math.min(from, Math.ceil(end)); old++) {
      covered.push([old, Math.min(end, old + 1) - Math.max(start, old)]);
    }
    return covered;
  });
}
