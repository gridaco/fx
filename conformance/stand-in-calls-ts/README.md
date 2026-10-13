# stand-in-calls-ts

Paid calls made from TypeScript node bodies ([protocol.md](../../spec/protocol.md) §6.1, §8.2),
answered offline by a TypeScript stand-in, `answer` in `in/stand_in.ts`
(`--stand-in stand_in.ts#answer`). The case needs Node 24 and the built `@grida/fx`, and no
Python.

| Step | Its body (`in/nodes/paint.ts`) | The stand-in answers |
|---|---|---|
| `paint` | `ctx.imageGenerate({prompt, size: "64x64", background: "opaque"})`, and returns the answer's `image`, an `InputFile` | a 64×64 blue PNG |
| `retouch` | `ctx.imageEdit` with the picture it was given as `image` and an 8×8 swatch it makes with `ctx.out.png` as a reference | a 64×64 orange PNG |
| `describe` | `ctx.structuredGenerate` with a schema written in the module and the edit as `context` | `{subject, width, height}`, the size read from the context file's facts |

## The pictures

`in/lib/png.ts` writes a PNG of one colour byte by byte, as stand-in-image's `stand_in.py` does:
the signature, `IHDR`, one `IDAT` holding a zlib stream of stored deflate blocks with an Adler-32
computed in place, and `IEND`, each chunk's CRC-32 from `node:zlib`. The bytes do not depend on
any zlib build. The stand-in imports it from its own folder, and `retouch`'s body imports it to
make the swatch.

## Against stand-in-image

`paint`'s request is `{"prompt": "a lantern on a quay", "size": "64x64", "background":
"opaque"}`, stand-in-image's, so its call key (`asked.json`) and the picture's digest
(`project-one.json`) are stand-in-image's too: a body's call and a built-in step's call with the
same request are the same call, whichever host makes it.

## What the case pins

- A body's files travel in requests as file values: the `InputFile` by its digest, and the
  `Output` after the host stores it with `file.put` (`asked.json`, where each file the stand-in
  was handed has its name, kind, size and facts).
- The answers come back to the body as a `CallResult` (`result.image`, `result.json`), and the
  node fact `cost_usd` is 0: paid, at nothing.
- runs/two is answered from the stand-in store's result cache (`count.txt` stays 3, every
  instance a `hit`), and the project's own store gets no file; runs/three, without a stand-in,
  refuses the paid call without `--live`.
