# facts

`in/art.png` is a 3x2 RGBA PNG of 82 bytes, sha256
`43c45622827052254ce56ff4e896e5f49537dee31c0fae88b3af2bfe378aa6de`. Five pixels are fully
opaque and the last one (row 2, column 3) has alpha 128, so `has_alpha` is true and `opaque` is
false. `in/note.txt` is the 6 bytes `hello\n`.

The PNG was written by a few lines of Python using only the standard library: the signature,
an `IHDR` chunk (width 3, height 2, bit depth 8, color type 6, no interlace), one `IDAT` chunk
holding `zlib.compress(rows, 9)` where each row is a filter byte `0` followed by its RGBA
pixels, and an empty `IEND` chunk:

```python
import struct
import zlib


def chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))


rows = [
    [(255, 0, 0, 255), (0, 255, 0, 255), (0, 0, 255, 255)],
    [(255, 255, 255, 255), (0, 0, 0, 255), (128, 128, 128, 128)],
]
raw = b"".join(b"\x00" + b"".join(bytes(px) for px in row) for row in rows)
png = (
    b"\x89PNG\r\n\x1a\n"
    + chunk(b"IHDR", struct.pack(">IIBBBBB", 3, 2, 8, 6, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(raw, 9))
    + chunk(b"IEND", b"")
)
```

The committed bytes are the fixture; regenerating them with another zlib may produce different
(equally valid) bytes and a different digest.

## What the case pins

- `describe` is planned (`has_alpha` is true) and its prompt, in the `with` values of
  `expand.json`, renders as `3x2, 82 bytes, opaque false; 6 bytes of text/plain`.
- `flat` is absent (`opaque` is false).
- `wider` reads a fact of a file `describe` has not made yet, so its identity is null.
- File facts enter an identity only through the values they render into (`identity.json`).
  A file's kind is not part of its plain projection (`{"file": digest}`); it enters
  `describe#1`'s identity only as the file fact the prompt reads, `facts(inputs.note).kind`
  (`spec/identity.md` section 4).
