# stand-in-image-ts

[`stand-in-image`](../stand-in-image/) with its stand-in written in TypeScript: `answer` in
`in/stand_in.ts`, given as `--stand-in stand_in.ts#answer`. The engine starts a stand-in host of
the file's language, the TypeScript node host, in its working directory, the project the case
runs in ([protocol.md](../../spec/protocol.md) §5.7). The case needs Node 24 and the built
`@grida/fx`, and no Python: the workflow's one step is a paid built-in.

## The stand-in

`stand_in.ts` does what `stand_in.py` does, line for line:

- It keeps `count.txt` and `asked.json` as stand-in-image's README describes them. Entries are
  sorted by their JSON text as Python's `json.dumps(entry, sort_keys=True)` writes it, so the two
  stand-ins sort alike.
- It answers with the same 64×64 RGB PNG of one colour, `(40, 90, 160)`: the signature, `IHDR`,
  one `IDAT` holding a zlib stream of stored deflate blocks it writes itself, with the Adler-32
  it computes itself, and `IEND`; each chunk's CRC-32 is `node:zlib`'s `crc32`. Its bytes are
  stand_in.py's, whatever zlib either runtime was built with.

## Against stand-in-image

Every expected file is stand-in-image's, byte for byte. The plan digest, the call key, the
answer's digest and every record are the engine's, and none of them names the stand-in's file or
language: only the run's summary line `stand-in  stand_in.ts#answer`, which the case pins with
`mentions`, does.
