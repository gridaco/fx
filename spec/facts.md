# File facts

File facts are values the engine computes from a file's bytes. Node hosts never compute them ([protocol.md](protocol.md)): the expression `facts(f)` reads them, while planning for workflow inputs and project files and while running for a step's outputs, and every file ref the engine sends to a host carries them ([protocol.md](protocol.md) §3.1). They are not node facts, the values a body reports about its own result with `fact()`. This document defines every file fact exactly, so that any implementation computes the same values from the same bytes. [identity.md](identity.md) §4 defines file kinds and summarises these rules.

File facts enter an identity only through the values an expression renders from them (`facts(x).width` in a `with:` value, or an `if:` that decides whether a step is planned). A change to a rule here therefore changes those identities, and needs a migration note like any change to identity.

The words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

## 1. Every file

- **Members.** `facts(f)` is an object. It always has `bytes`, the file's size in bytes, and `kind`, the file's kind from its suffix ([identity.md](identity.md) §4). The kind decides what else it has: the image kinds (§2), `audio/wav` (§3), `video/mp4` (§4), `video/webm` and `video/x-matroska` (§5). Every other kind, `audio/mpeg`, `audio/ogg` and `file` among them, has `bytes` and `kind` only.
- **Order.** Members are written in this order: `bytes`, `kind`, then for images `width`, `height`, `has_alpha`, `opaque`; for WAV `duration`; for video `width`, `height`, `fps`, `duration`, `frames`, `has_alpha`. Only display depends on it: canonical JSON sorts keys ([identity.md](identity.md) §2).
- **Absent members.** A member the rules below do not give for a file is left out, never `null`. An expression that reads it fails (`no field 'fps'`).
- **Numbers** are FX numbers ([identity.md](identity.md) §1): a whole number is an integer, so a rate of 24 frames per second is written `24`, not `24.0`.
- **Rounding to 6 places.** Where a rule says a number is *rounded to 6 places*, the value is the binary64 number nearest to the decimal of 6 fractional digits that is nearest to the exact value of the binary64 input, a tie going to the even last digit. Rust's `format!("{:.6}", x)` read back as `f64`, and Python's `round(x, 6)`, both compute it: 1/128 gives 0.007812, 3/128 gives 0.023438, 1/44100 gives 0.000023.
- **Seconds from units.** `u` units of a timescale `s` (each unit 1/`s` seconds) are `u` converted to binary64 (to nearest, ties to even), divided by `s` in binary64, and rounded to 6 places.
- **Rates from fractions.** A rate *p*/*q* of whole numbers is *p* and *q* each converted to binary64 (to nearest, ties to even), *p* divided by *q* in binary64, and rounded to 6 places.
- **Only the bytes and the kind.** Facts are a function of a file's bytes and its kind alone. An implementation MUST NOT run an external program to compute them, and MUST compute the same facts wherever it runs.
- **Bounded reading.** An implementation MUST check every size a file declares against the bytes present before using it, MUST NOT allocate memory in proportion to a declared size, and MUST NOT crash on any input: every file gives facts or a refusal. The rules read a file's headers, the boxes, elements and chunks they name, and at most the start of one frame; a video's other frames and a WAV's samples are never read, so an implementation need not hold a file in memory to compute its facts.
- **Refusals.** A file whose bytes do not decode under its kind's rule is *refused*. An expression that reads its facts fails with `<file name>: <reason>`, so the step or plan that evaluated it fails the way any expression error does. Each kind's reason starts with a fixed prefix: `not a PNG picture (`, `not a GIF picture (`, `not a WebP picture (`, `not a JPEG picture (`, `not an MP4 file (`, `not a Matroska or WebM file (`. The prefix is normative; the text in parentheses is informative and names the first problem found. A WAV file is never refused (§3). A file ref the engine hands a host for a refused file carries `bytes` and `kind` only ([protocol.md](protocol.md) §3.1).

## 2. Images

Images (`image/png`, `image/jpeg`, `image/webp`, `image/gif`) add:

- `width`, `height`: pixels of the first frame;
- `has_alpha`: true when the encoding can carry transparency: an alpha channel, a PNG `tRNS` chunk, a WebP alpha flag, or a GIF transparent index;
- `opaque`: true when `has_alpha` is false or every pixel of the first frame is fully opaque.

Per format:

- **PNG.** `width` and `height` come from `IHDR`. `has_alpha` is true when the color type has an alpha channel (greyscale with alpha, RGBA) or the file has a `tRNS` chunk. Only then is the image of the `IDAT` chunks decoded, with `tRNS` applied and low bit depths expanded, and `opaque` is true when every pixel's alpha is at its maximum: 255 at 8 bits, 65535 at 16 bits. The image of an animated PNG is its default image, the `IDAT` one. A chunk whose CRC does not match refuses the file when it is critical and is ignored when it is ancillary (a `tRNS` chunk among them). A file whose header (every chunk up to the first `IDAT`) does not decode is refused, and so is a file with alpha whose image data does not decode completely, whatever its first pixels hold.
- **GIF.** `width` and `height` are the logical screen's. `has_alpha` is true when the first frame's graphic control extension names a transparent index, and `opaque` is true when none of that frame's own pixels uses it. Pixels of the logical screen that the first frame does not cover are not looked at, and later frames are never read.
- **WebP.** `width` and `height` are the canvas's. `has_alpha` is the alpha flag (the `VP8X` alpha flag, or the alpha hint of a lossless `VP8L` image). With alpha, the first frame is decoded and `opaque` is true when every alpha is 255.
- **JPEG.** `width` and `height` come from the first start-of-frame marker (`SOF0` to `SOF15`, except `DHT`, `JPG` and `DAC`), found by walking the marker segments from the start-of-image marker. A JPEG never has alpha: `has_alpha` is false and `opaque` true. An EXIF orientation is not applied: the size is the stored one. A frame header with a width or height of 0 is refused.

FX judges images by this rule as written. stage-gen's engine read them with Pillow, which differs on these files, so `opaque` can change for them:

| File | stage-gen's engine | FX |
|---|---|---|
| 16-bit alpha, every alpha `0xFF00` | `opaque` true: alpha judged on its high byte | `opaque` false: judged at full depth, only 65535 is opaque |
| 16-bit greyscale or 16-bit RGB with a `tRNS` colour every pixel has | `opaque` true: the `tRNS` chunk was ignored | `opaque` false: `tRNS` always applies |
| GIF whose first frame does not cover the screen, with a transparent index the frame does not use | `opaque` false: the uncovered area counts as transparent | `opaque` true: only the frame's own pixels count |
| bytes that do not decode (a truncated PNG, a text file named `.png`) | facts without `opaque`, or an error that stopped planning | refused |

## 3. WAV

`audio/wav` adds `duration`, in seconds rounded to 6 places, when the file is a PCM RIFF/WAVE file as the rules below say. Otherwise it has `bytes` and `kind` only. A WAV file is never refused. Every field is little-endian.

1. Bytes 0–3 are `RIFF`, bytes 4–7 the RIFF size *R*, and bytes 8–11 `WAVE`, with *R* at least 4. The *body* is the *R* bytes from byte 8; *R* may claim more bytes than the file holds. `RF64`, `BW64` and `RIFX` files have no duration.
2. Chunks are walked in order from body offset 4. Each begins with an 8-byte header: a 4-byte id and a 4-byte size *S*. A header that does not lie wholly inside the body (its end at most *R*) and inside the file ends the walk. After a chunk that is not `data`, the next one starts after its *S* bytes of content and one pad byte when *S* is odd; when that start is past the end of the body (beyond *R*), there is no duration.
3. A `fmt ` chunk's content is read as far as *S*, the body and the file allow. It needs 16 bytes: format tag (2 bytes), channels (2), frame rate (4), byte rate (4), block align (2), bits per sample (2). The format tag MUST be 1 (PCM), or `0xFFFE` (extensible) with at least 40 bytes whose bytes 24–39 are the PCM sub-format `01 00 00 00 00 00 10 00 80 00 00 AA 00 38 9B 71`. Channels and bits per sample MUST each be at least 1. Otherwise there is no duration: other formats (3, IEEE float; 6, A-law; 7, µ-law; an extensible float sub-format) and chunks cut short alike. A later `fmt ` chunk replaces an earlier one, and must itself pass.
4. The first `data` chunk ends the walk. With no `fmt ` chunk before it, there is no duration. The frame count is *S* ÷ (channels × ⌈bits per sample ÷ 8⌉), dividing integers and dropping the remainder, where *S* is the size the header declares, not the bytes present: a streaming writer's `0xFFFFFFFF` counts as written. The byte rate and the block align are not used.
5. A frame rate of 0 gives no duration. Otherwise `duration` is the frame count divided by the frame rate (both exact in binary64, so the quotient is correctly rounded), rounded to 6 places.
6. A walk that ends without a `data` chunk gives no duration.

These are the rules of the `wave` module of CPython 3.12, which stage-gen's engine used. Where that module raised an error the engine did not catch, FX gives no duration (§7).

## 4. MP4

`video/mp4` files are read as the ISO base media file format (ISO/IEC 14496-12). A file with a video track adds `width`, `height`, `fps`, `duration`, `frames` and `has_alpha`, each as below. Every field is big-endian. Refusals read `not an MP4 file (<reason>)`.

### 4.1 Boxes

- A box is a 4-byte size, a 4-byte type and its content. A size of 1 means a 64-bit size follows the type, so the header is 16 bytes; a size of 0 means the box runs to the end of its parent, or of the file at the top level. A header cut short, a size smaller than the header, and a box that runs past the end of its parent or of the file are refused.
- The top-level boxes are read to the end of the file. The first `moov` is the movie, and every `moof` is a fragment. A file without a `moov` is refused (`it has no moov box`).
- Only the boxes these rules name are read. Where several siblings share a type, the first counts, except `trak`, `moof`, `traf` and `trun`. These lists of boxes are parsed, and a box in them that does not parse refuses the file: the children of `moov`; of each `trak` up to the video track, its children and its `mdia`'s children; of the video track, its `minf`'s and `stbl`'s children, the first sample entry of its `stsd`, and its `edts`'s children when `mvhd` has a timescale above 0; when the video track has a `tkhd`, the children of the first `mvex` up to the first `trex` for the track, and the children of every `moof` and of each `traf` in them.
- These fields are read, and only these: each `hdlr`'s handler type up to the video track's; of the video track, its `mdhd`'s timescale and duration, its `stsd`'s entry count and its first sample entry's width and height, every `stts` entry, `stsz`'s sample size and sample count (or `stz2`'s sample count, content offset 8), its `tkhd`'s track ID, and the `elst` entries; `mvhd`'s timescale only when the video track has an `edts`, and its duration never; the fragment fields of §4.3; and what §4.4 reads: an `mp4v` entry's `esds`, and for a PNG track its first sample's offset and size. A version byte (content offset 0) is read wherever a field's offset depends on it.
- A field these rules read that lies beyond its box's content is refused (`its <type> box is too short`), and so is a table (`stts`, `elst`, `trun`) whose entry count claims more entries than its box holds. A version 1 `mvhd`, `mdhd`, `tkhd` or `elst` has 64-bit times and durations: the timescale at content offset 20 (else 12), `mdhd`'s duration at 24 (else 16), the track ID at 20 (else 12), and edits of 20 bytes (else 12), each a segment duration then a media time.

### 4.2 The video track

- The video track is the first `trak` in `moov` whose `mdia` has an `hdlr` whose handler type (content bytes 8–11) is `vide`. A file with no video track (an audio-only MP4) has no video facts: `bytes` and `kind` only.
- The video track MUST have an `mdhd` (else refused: `its video track has no mdhd box`), and an `stsd` in `mdia`/`minf`/`stbl` (`its video track has no stsd box`) whose first sample entry (`its video track has no sample entry`) has at least the 78 bytes of a visual sample entry's fields (`its video sample entry is too short`). The first sample entry is the first box after the `stsd`'s entry count (content bytes 4–7); an `stsd` whose entry count is 0 has none, whatever follows the count.
- `width` and `height` are the sample entry's (content offsets 24 and 26). `tkhd`'s width and height, which carry the pixel aspect ratio, its display matrix, which carries a rotation, and any `pasp` box are not used: the size is the coded picture's, unrotated.

### 4.3 Samples, rate and length

- **Timescale.** The media timescale is `mdhd`'s.
- **Fragments.** When the video track has a `tkhd`, every `traf` of every `moof` whose first `tfhd` names its track ID belongs to the track; a track with no `tkhd` has no fragments. A fragment's default sample duration is its `tfhd`'s when flag `0x08` is set (after the track ID, an 8-byte base data offset with flag `0x01` and a 4-byte sample description index with flag `0x02`), else the `default_sample_duration` of the first `trex` for the track in the first `mvex`, else 0. A `trun` holds its sample count, a 4-byte data offset with flag `0x001` and 4-byte first sample flags with flag `0x004`, then per sample a 4-byte field for each of the flags `0x100` (duration), `0x200` (size), `0x400` (flags) and `0x800` (composition offset) that is set. A sample's duration is its own when flag `0x100` is set, else the default.
- **Durations.** The track's sample durations are those its `stts` entries give (each entry a sample count and the duration of each of those samples), followed by those of its fragments' samples, in file order.
- `frames` is the sample count of `stsz` (or `stz2`) plus the sample count of every `trun` of the track's fragments. An edit list does not change it, so `frames ÷ fps` need not equal `duration`. `frames` is always given.
- `fps` is read from the track's sample durations above 0, in order. With none, or a timescale of 0, it is absent; with one, *d*, it is timescale ÷ *d*. Otherwise the last duration is left out (it only says when the track ends), and so is the first when another is left besides it and it differs from the second by more than half the second (a first frame that starts early or late, as behind audio). With 2^64 or more durations left, `fps` is absent. Of the durations left, let *m* be the smallest, *g* their greatest common divisor, *n* their count and *s* their sum:
  - When *m* equals *g*, every duration is a multiple of the smallest, and `fps` is timescale ÷ *m*: a constant rate, or a variable one whose steps are multiples of its shortest (which gives the rate of its shortest step, not frames over duration).
  - Otherwise the durations are taken for a constant rate whose timestamps were rounded to a grid of *g* units, and the rates they allow are those strictly between timescale × *n* ÷ (*s* + *g*) and timescale × *n* ÷ (*s* − *g*). `fps` is the whole number among them nearest timescale × *n* ÷ *s*, the smaller of two equally near; without one, the whole multiple of 1000/1001 among them nearest it, likewise; without one, the fraction among them with the smallest denominator (exactly one has it). At a timescale of 1000, 24 frames a second are written 42, 41, 42, 42, 41, … units apart and give 24; 30000/1001 frames a second over 120 frames give 30000/1001; at 10^7, 416666 and 416667 give 24.

  The rate is rounded as §1 says for a fraction: timescale ÷ *d* is the fraction timescale/*d*, and a whole multiple *k* of 1000/1001 is 1000*k*/1001.
- `duration`: the track's length in units is `mdhd`'s duration, except that it is the sum of every sample's duration (those of 0 included) when the track has fragments, or when `mdhd`'s duration is 0 or all ones (unknown). Then, when the track's `edts` has an `elst` with at least one non-empty edit (a media time other than −1) whose segment duration is above 0, and `mvhd` has a timescale above 0, the length becomes the smaller of itself and the sum, over those edits, of each segment duration rescaled from the movie timescale to the media timescale and rounded to nearest, halves up. `duration` is that length in seconds (§1); it is absent when the timescale is 0. `mvhd`'s duration is never read, and its timescale only for a video track with an `edts`: an audio-only file, or a track without edits, never needs it.

### 4.4 has_alpha

`has_alpha` comes from the first sample entry's type:

| Sample entry | `has_alpha` |
|---|---|
| `avc1`, `avc2`, `avc3`, `avc4`, `hvc1`, `hev1`, `av01`, `vp08`, `vp09` | false |
| `mp4v` whose `esds` `objectTypeIndication` is `0x20` (MPEG-4 Visual), `0x60`–`0x65` (MPEG-2 Visual), `0x6A` (MPEG-1 Visual) or `0x6C` (JPEG) | false |
| `mp4v` with `objectTypeIndication` `0x6D` (PNG), and `png ` | the first sample read as a PNG picture: true when its color type has alpha or it has a `tRNS` chunk before its image data (§2) |
| anything else | absent |

The `esds` box is the first `esds` among the sample entry's child boxes, after its 78 bytes of fields. Its content is 4 bytes of version and flags, then at content offset 4 the ES descriptor (tag 3). A descriptor is a tag byte, then its size in one to four bytes, each with a continuation bit (`0x80`) that, when set, means another follows; a fourth byte ends the size whatever its bit, and the size itself is not used. The ES descriptor's content is a 2-byte ES ID and a flags byte, then 2 bytes when flag `0x80` is set (`dependsOn_ES_ID`), a length byte and that many bytes when `0x40` is set (a URL), and 2 bytes when `0x20` is set (`OCR_ES_Id`); then comes the decoder configuration descriptor (tag 4), whose first content byte is the `objectTypeIndication`. When there is no `esds`, the child boxes do not parse, a tag is not the one expected there, or a byte these rules read lies beyond the `esds` box's content, `has_alpha` is absent; none of these refuses the file.

The first sample of a PNG track starts at the first offset of `stco` (or `co64`) and is as long as `stsz` says: its sample size, or when that is 0 its first entry (content offset 12). Only a PNG track reads them, and only when `stsz`'s sample count is above 0; a field of them beyond its box refuses the file (§4.1). When the sample does not lie within the file, or the track has no `stsz` (an `stz2` instead) or no samples outside fragments, or its PNG header does not decode, `has_alpha` is absent.

## 5. Matroska and WebM

`video/webm` and `video/x-matroska` files are read as EBML (RFC 8794) with the Matroska elements below. A file with a video track adds `width`, `height`, `fps`, `duration`, `frames` and `has_alpha`. Refusals read `not a Matroska or WebM file (<reason>)`.

### 5.1 Elements

- An element is an ID of 1 to 4 bytes, its length given by the leading zero bits of its first byte and its marker bits kept; a size, a variable-size integer of 1 to 8 bytes whose marker bit is removed, where all value bits set mean an unknown size; and its content. An ID whose first byte is below `0x10`, a size whose first byte is 0, a header cut short, and an element that runs past the end of its parent or of the file are refused.
- The file MUST start with the EBML header `1A 45 DF A3` (`it does not start with an EBML header`), of known size. Its `DocType` (the first `0x4282` child; `matroska` when there is none; trailing NUL bytes ignored) MUST be `matroska` or `webm`.
- The top-level elements after the header are read up to the first `Segment` (`0x18538067`), which may have an unknown size and then runs to the end of the file. Any other top-level element of unknown size is refused, and so is a file with no `Segment`.
- The Segment's children are read to its end. A `Cluster` may have an unknown size: it then ends where the next element whose ID is a level-1 ID (`SeekHead`, `Info`, `Tracks`, `Cluster`, `Cues`, `Attachments`, `Chapters`, `Tags`, or an EBML header or `Segment`) starts, or at the end of the Segment. Any other element of unknown size is refused.
- Only these elements are read: the first `Info` (`TimecodeScale`, `Duration`); the first `Tracks` and its `TrackEntry`s (`TrackNumber`, `TrackType`, `CodecID`, `CodecPrivate`, `DefaultDuration`, and the first `Video`'s `PixelWidth` and `PixelHeight`); every `Cluster` (its `SimpleBlock`s, and the `Block`s of its `BlockGroup`s). The first of each element counts. These lists of elements are parsed, and an element in them that does not parse refuses the file: the EBML header's children up to its `DocType`; the top-level elements up to the `Segment`; the Segment's children; the children of the first `Info`; the children of the first `Tracks` up to the video track's `TrackEntry`, of each `TrackEntry` among them, and of its first `Video`; the children of every `Cluster` and of each `BlockGroup` in them. An unsigned integer longer than 8 bytes and a float that is not 0, 4 or 8 bytes long are refused where they are read.

### 5.2 The video track, rate and length

- The video track is the first `TrackEntry` of the first `Tracks` whose `TrackType` is 1. A file with no video track has no video facts: `bytes` and `kind` only.
- `width` and `height` are `PixelWidth` and `PixelHeight`. A video track without both is refused (`its video track has no pixel size`), and so is one whose size does not fit in 32 bits.
- `fps` comes from `DefaultDuration` *D*, the nanoseconds per frame. When *D* is above 0, take the fraction *p*/*q* closest to 10⁹/*D* whose denominator *q* is at most 1001; of two fractions equally close, the one with the smaller denominator. When *p* is above 0, `fps` is *p*/*q* rounded to 6 places; otherwise, and without `DefaultDuration`, it is absent. So 41666666 ns gives 24, 41708333 gives 24000/1001 (23.976024), 33366667 gives 30000/1001 (29.97003) and 16683333 gives 60000/1001 (59.94006). (The closest fraction is the last convergent of the continued fraction of 10⁹/*D* whose denominator is at most 1001, or the semiconvergent after it, whichever is closer.)
- `duration` comes from the first `Info`'s `Duration`, in units of `TimecodeScale` nanoseconds (1000000 when `TimecodeScale` is absent). When `Duration` is a finite number above 0 and `TimecodeScale` is above 0, the microseconds are ((`Duration` × `TimecodeScale`) × 1000) ÷ 1000000, each step in binary64, truncated toward zero; `duration` is that many microseconds in seconds (÷ 10⁶), rounded to 6 places. Otherwise it is absent. Block timestamps, `Cues` and track durations are not used.
- `frames` counts the frames of the video track's blocks (those whose track number is its `TrackNumber`) in every `Cluster`: a `SimpleBlock` or `Block` without lacing (flags bits `0x06` clear) is one frame; a laced one is its lace count byte plus one. Every block's header is read, whatever its track: its track number (a variable-size integer, §5.1, marker removed), 2-byte timecode, flags, and when laced its lace count and lace sizes (§5.3). A block too short for them is refused (`a block at byte N is too short`), and so is one whose track number or EBML lace size is a variable-size integer whose first byte is 0 (`a block at byte N is not valid`). `frames` is always given.

### 5.3 has_alpha

`has_alpha` comes from `CodecID`:

| `CodecID` | `has_alpha` |
|---|---|
| `V_VP8`, `V_VP9` | false, even with `AlphaMode` 1: the alpha is a separate stream in block additions, not part of the codec's picture |
| `V_AV1`, `V_MPEG4/ISO/AVC`, `V_MPEGH/ISO/HEVC`, `V_MPEG4/ISO/SP`, `V_MPEG4/ISO/ASP`, `V_MPEG4/ISO/AP`, `V_MPEG1`, `V_MPEG2`, `V_THEORA`, `V_MJPEG` | false |
| `V_FFV1` | its transparency flag (§5.4) |
| `V_MS/VFW/FOURCC` with a `CodecPrivate` of at least 40 bytes (a whole `BITMAPINFOHEADER`) whose compression (bytes 16–19) is `MPNG`, `PNG1` or `png ` | the first frame read as a PNG picture: true when its color type has alpha or it has a `tRNS` chunk before its image data (§2) |
| `V_MS/VFW/FOURCC` with a `CodecPrivate` of at least 40 bytes whose compression is `FFV1` | its transparency flag (§5.4), with the configuration record after the `BITMAPINFOHEADER` (from byte `biSize`, bytes 0–3, when that is at least 40 and at most the `CodecPrivate`'s length; else none) |
| `V_MS/VFW/FOURCC` with a shorter or no `CodecPrivate`, or another compression | absent |
| anything else (`V_PRORES`, `V_QUICKTIME`, `V_UNCOMPRESSED`, …) | absent |

JPEG has no alpha in either container: `V_MJPEG` here, and an `mp4v` entry whose `objectTypeIndication` is `0x6C` in MP4 (§4.4).

The *first frame* is the first frame of the video track's first block in file order: the bytes after the block's track number, timecode and flags and, in a laced block, after its lace count and lace sizes, as long as the first lace, and never past the block's end. The lace sizes give every frame's size but the last: Xiph lacing, each a run of 255s and a byte below 255, summed; EBML lacing, the first a variable-size integer (§5.1, marker removed) and each of the others but the last a signed difference coded the same way (only their lengths are needed); fixed-size lacing has none, and the first lace is the block's frame bytes divided by the frame count, rounded down. When there is no first frame or its PNG header does not decode, `has_alpha` is absent.

### 5.4 The FFV1 transparency flag

An FFV1 stream (RFC 9043) carries the flag in its configuration record, the `CodecPrivate`, from version 2, and in the header of its key frames in versions 0 and 1:

- **With a configuration record** (a `CodecPrivate` that is not empty), the record's `version` MUST be 2 or 3, and a version 3 record MUST be at least 4 bytes long and pass its CRC: the CRC-32 with polynomial `0x04C11DB7`, most significant bit first, starting from 0 and not complemented, over the whole record, its last four bytes included, is 0. The last four bytes of a version 3 record are not range coded. A record that does not pass gives no `has_alpha`; the frames are not read then.
- **Without one**, the first frame MUST be a key frame (its first bit, read with its own state of 128, is 1) whose `version` is 0 or 1.

Then the fields are read with FFV1's range decoder and one array of 32 states, all starting at 128, in this order: `version`; `micro_version` (version 3); `coder_type`, and when it is 2, 255 signed values of a custom state table, read and set aside; `colorspace_type`; `bits_per_raw_sample` (version 1 and later); the `chroma_planes` bit; `log2_h_chroma_subsample`; `log2_v_chroma_subsample`; and the transparency bit (`extra_plane`), which is `has_alpha`.

The range decoder starts with *low* the first two bytes (big-endian; when there are fewer, *low* is 0 and two bytes count as read past the end) and *range* `0xFF00`; when *low* is at least `0xFF00` it becomes `0xFF00` and no further bytes are read. A bit with state *s* splits off *r* = (*range* × *s*) >> 8: *range* becomes *range* − *r*; if *low* < *range* the bit is 0 and *s* becomes `zero_state[s]`, else *low* becomes *low* − *range*, *range* becomes *r*, the bit is 1 and *s* becomes `one_state[s]`. After each bit, when *range* is below `0x100`, both shift left by 8 bits and the next byte is added to *low* (nothing past the end of the bytes). `one_state` is RFC 9043's default state transition table, and `zero_state[i]` = 256 − `one_state[256 − i]`. A value is 0 when its first bit (state 0) is 1; otherwise an exponent *e* counts the 1 bits read with states 1 + min(*e*, 9), the value starts at 1 and takes, for *i* from *e* − 1 down to 0, one bit with state 22 + min(*i*, 9), and a signed value then reads its sign with state 11 + min(*e*, 10).

`has_alpha` is absent when the record or the frame does not pass, an exponent exceeds 31, or more than 2 bytes are read past the end.

## 6. Fixtures

The fixtures under [vectors/facts/](vectors/facts/) pin these rules. `tools/make_fixtures.py` made every file: WAV with Python's `wave` and `struct` modules, video with ffmpeg 9.0.1 from its `testsrc` picture (64×48 unless stated) and `sine` sound, written with `-fflags +bitexact -flags:v +bitexact -map_metadata -1`, H.264 with x264 in yuv420p with its version message removed (`-bsf:v filter_units=remove_types=6`). The committed bytes are the fixtures: another encoder build writes other bytes, so `make_fixtures.py --check` re-reads the committed files with its own Python readers of these rules and never re-encodes. Each folder's `expected.json` holds every file's facts, `bytes` included, or its refusal; an implementation MUST compute exactly those values. The tables below give the facts after `bytes` and `kind`.

**`wav/`** (`audio/wav`):

| File | Made as | Facts |
|---|---|---|
| `pcm16_8k.wav` | `wave`: 8000 Hz, mono, 16 bits, 1000 frames | `duration` 0.125 |
| `pcm16_44k.wav` | `wave`: 44100 Hz, mono, 16 bits, 1234 frames | `duration` 0.027982 |
| `pcm16_48k_stereo.wav` | `wave`: 48000 Hz, stereo, 16 bits, 960 frames | `duration` 0.02 |
| `pcm8.wav` | `wave`: 4000 Hz, 8 bits, 2000 frames | `duration` 0.5 |
| `pcm24.wav` | `wave`: 96000 Hz, 24 bits, 480 frames | `duration` 0.005 |
| `one_frame.wav` | `wave`: 44100 Hz, 1 frame (2.2676 × 10⁻⁵ s) | `duration` 0.000023 |
| `three_hz.wav` | `wave`: 3 Hz, 1 frame | `duration` 0.333333 |
| `no_frames.wav` | `wave`: 22050 Hz, no frames | `duration` 0 |
| `extensible_pcm.wav` | extensible format with the PCM sub-format, 44100 Hz, stereo, 16 bits, 441 frames | `duration` 0.01 |
| `extensible_float.wav` | extensible format with the IEEE float sub-format | none |
| `float.wav` | format tag 3 | none |
| `mp3_named.wav` | an ID3 header and zeros | none |
| `empty.wav` | no bytes | none |
| `rate_zero.wav` | a frame rate of 0 | none |
| `truncated_fmt.wav` | a `fmt ` chunk of 8 bytes and no `data` | none |
| `declared_longer.wav` | a `data` chunk declaring 96000 stereo frames at 48000 Hz, with no sample bytes | `duration` 2 |
| `partial_frame.wav` | 5 data bytes of 2-byte frames at 8000 Hz | `duration` 0.00025 |
| `data_size_unknown.wav` | a `data` size of `0xFFFFFFFF` at 8000 Hz, 16 bits | `duration` 268435.455875 |
| `riff_size_unknown.wav` | a RIFF size of `0xFFFFFFFF`, 400 frames at 8000 Hz | `duration` 0.05 |
| `riff_size_cuts_data_header.wav` | a RIFF size that ends inside the `data` chunk's header | none |
| `odd_chunk.wav` | a 3-byte `LIST` chunk and its pad byte before `fmt ` | `duration` 0.05 |
| `odd_chunk_unpadded.wav` | the same without the pad byte | none |
| `data_before_fmt.wav` | `data` before `fmt ` | none |
| `two_fmt_chunks.wav` | a `fmt ` at 8000 Hz, then one at 16000 Hz, then 400 frames | `duration` 0.025 |
| `rf64.wav` | `RF64` in place of `RIFF` | none |
| `zero_channels.wav` | 0 channels | none |

**`mp4/`** (`video/mp4`, H.264 unless stated):

| File | Made as | Facts |
|---|---|---|
| `h264_24fps.mp4` | 24 fps, 48 frames, with B-frames | 64×48, `fps` 24, `duration` 2, `frames` 48, `has_alpha` false |
| `h264_2997.mp4` | 30000/1001 fps, 30 frames | `fps` 29.97003, `duration` 1.001, `frames` 30 |
| `h264_25fps_no_bframes.mp4` | 25 fps, 10 frames, `-bf 0` | `fps` 25, `duration` 0.4, `frames` 10 |
| `h264_60fps.mp4` | 60 fps, 30 frames | `fps` 60, `duration` 0.5, `frames` 30 |
| `h264_one_frame.mp4` | 1 frame (an edit of 42 ms, longer than the frame) | `fps` 24, `duration` 0.041667, `frames` 1 |
| `h264_timescale_600.mp4` | `-video_track_timescale 600`, 24 frames | `fps` 24, `duration` 1, `frames` 24 |
| `h264_66x50.mp4` | 66×50, 12 frames | 66×50, `fps` 24, `duration` 0.5, `frames` 12 |
| `h264_yuv444.mp4` | yuv444p, 24 frames | `fps` 24, `duration` 1, `frames` 24, `has_alpha` false |
| `faststart.mp4` | `-movflags +faststart` (`moov` first), 24 frames | `fps` 24, `duration` 1, `frames` 24 |
| `audio_first.mp4` | an AAC track before the video track, 1 s | `fps` 24, `duration` 1, `frames` 24 |
| `audio_only.mp4` | AAC only | none |
| `fragmented.mp4` | `-movflags +frag_keyframe+empty_moov`, 24 frames | `fps` 24, `duration` 1, `frames` 24 |
| `fragmented_two.mp4` | the same with `+default_base_moof` and two fragments (`-g 12`) | `fps` 24, `duration` 1, `frames` 24 |
| `fragmented_moov_samples.mp4` | `-movflags +frag_keyframe`, `-g 12`: 12 samples in `moov` (its `mdhd` says 0.5 s) and 12 in a fragment | `fps` 24, `duration` 1, `frames` 24 |
| `fragmented_zero_edit.mp4` | `+frag_keyframe+empty_moov+delay_moov`: an edit with a segment duration of 0 | `fps` 24, `duration` 1, `frames` 24 |
| `vfr.mp4` | 12 frames 1/24 s apart, then 12 frames 1/12 s apart (`setpts`, `-fps_mode vfr`) | `fps` 24, `duration` 1.375, `frames` 24 |
| `h264_ts1000_24fps.mp4` | `-video_track_timescale 1000`, 48 frames: durations 42, 41, 42, … | `fps` 24, `duration` 2, `frames` 48 |
| `h264_ts1000_2997.mp4` | 30000/1001 fps at `-video_track_timescale 1000`, 120 frames: durations 33, 34, 33, … | `fps` 29.97003, `duration` 4.004, `frames` 120 |
| `h264_ts1000000_30fps.mp4` | 30 fps at `-video_track_timescale 1000000`, 31 frames: durations 33334, 33333, … | `fps` 30, `duration` 1.033333, `frames` 31 |
| `h264_ts90000_5994.mp4` | 60000/1001 fps at `-video_track_timescale 90000`, 60 frames: durations 1502, 1501, … | `fps` 59.94006, `duration` 1.001011, `frames` 60 |
| `ismv.mp4` | `-f ismv` (fragments at a timescale of 10^7), 24 frames: durations 416667, 416666, … | `fps` 24, `duration` 1, `frames` 24 |
| `fragmented_av.mp4` | an AAC track before the video track, `-g 12 -movflags +frag_keyframe+empty_moov`, 24 frames: the first lasts 1061 units, the others 512 | `fps` 24, `duration` 1.044678, `frames` 24 |
| `remux_ms.mp4` | `-c copy` of 30 fps H.264 in Matroska (millisecond timestamps), 30 frames: durations 528, 544, … at a timescale of 16000 | `fps` 30, `duration` 1, `frames` 30 |
| `mjpeg.mp4` | Motion JPEG (`mp4v`, `objectTypeIndication` `0x6C`), 3 frames | `fps` 24, `duration` 0.125, `frames` 3, `has_alpha` false |
| `mvhd_short.mp4` | `h264_25fps_no_bframes.mp4` with its `mvhd` cut after its timescale, a `free` box in the bytes it gave up | `fps` 25, `duration` 0.4, `frames` 10 |
| `mvhd_short_audio.mp4` | `audio_only.mp4` with its `mvhd` cut the same way | none |
| `mvhd_no_timescale.mp4` | `h264_25fps_no_bframes.mp4` with its `mvhd` cut before its timescale; its track has an edit list | refused: `not an MP4 file (its mvhd box is too short)` |
| `stsz_no_table.mp4` | `h264_25fps_no_bframes.mp4` with its `stsz` (sample size 0) cut after its sample count, a `free` box after it | `fps` 25, `duration` 0.4, `frames` 10 |
| `stsd_count_zero.mp4` | `h264_25fps_no_bframes.mp4` with its `stsd` entry count set to 0, its sample entry kept | refused: `not an MP4 file (its video track has no sample entry)` |
| `edit_cut.mp4` | `-ss 0.5 -c copy` of `h264_24fps.mp4`: an edit list cuts 0.5 s | `fps` 24, `duration` 1.5, `frames` 48 |
| `empty_edit.mp4` | `-output_ts_offset 0.5`: an empty edit of 0.5 s, then 1 s of media | `fps` 24, `duration` 1, `frames` 24 |
| `rotated.mp4` | `-display_rotation 90 -c copy` of `h264_24fps.mp4` | 64×48, `fps` 24, `duration` 2, `frames` 48 |
| `sar_2_1.mp4` | `setsar=2` (`tkhd` says 128×48) | 64×48, `fps` 24, `duration` 1, `frames` 24 |
| `mpeg4_part2.mp4` | MPEG-4 Part 2 (`mp4v`, `objectTypeIndication` `0x20`), 24 frames | `fps` 24, `duration` 1, `frames` 24, `has_alpha` false |
| `png_rgba.mp4` | PNG frames in RGBA (`mp4v`, `objectTypeIndication` `0x6D`), 3 frames | `fps` 24, `duration` 0.125, `frames` 3, `has_alpha` true |
| `png_rgb.mp4` | PNG frames in RGB, 3 frames | `fps` 24, `duration` 0.125, `frames` 3, `has_alpha` false |
| `quicktime.mov` | 24 frames written as QuickTime | kind `file`: none |
| `truncated.mp4` | the first 2000 bytes of `h264_24fps.mp4` | refused: `not an MP4 file (a box at byte 40 runs past the end of the file)` |
| `garbage.mp4` | the 11 bytes `not a video` | refused: `not an MP4 file (a box at byte 0 runs past the end of the file)` |

Every MP4 above with a video track is 64×48 unless stated, and `has_alpha` false unless stated.

**`matroska/`** (`video/x-matroska` for `.mkv`, `video/webm` for `.webm`; every one 64×48 at 24 fps unless stated):

| File | Made as | Facts |
|---|---|---|
| `h264.mkv` | H.264, 24 frames | `fps` 24, `duration` 1, `frames` 24, `has_alpha` false |
| `vp9.webm` | VP9 (libvpx), 24 frames | `fps` 24, `duration` 1, `frames` 24, `has_alpha` false |
| `vp9_5994.webm` | VP9 at 60000/1001 fps (`DefaultDuration` 16683333), 6 frames | `fps` 59.94006, `duration` 0.1, `frames` 6, `has_alpha` false |
| `vp9_alpha.webm` | VP9 in yuva420p: `AlphaMode` 1, alpha in block additions, 24 frames in `BlockGroup`s | `fps` 24, `duration` 1, `frames` 24, `has_alpha` false |
| `ffv1_yuva.mkv` | FFV1 in yuva420p (a version 3 record), 6 frames | `fps` 24, `duration` 0.25, `frames` 6, `has_alpha` true |
| `ffv1_yuv.mkv` | FFV1 in yuv420p, 6 frames | `fps` 24, `duration` 0.25, `frames` 6, `has_alpha` false |
| `ffv1_level1_yuva.mkv` | FFV1 version 1 in yuva420p (no record; the flag is in the key frame), 6 frames | `fps` 24, `duration` 0.25, `frames` 6, `has_alpha` true |
| `png_rgba.mkv` | PNG frames in RGBA (`V_MS/VFW/FOURCC`, `MPNG`), 6 frames | `fps` 24, `duration` 0.25, `frames` 6, `has_alpha` true |
| `png_rgba64.mkv` | PNG frames in 16-bit RGBA, 3 frames | `fps` 24, `duration` 0.125, `frames` 3, `has_alpha` true |
| `png_rgb.mkv` | PNG frames in RGB, 6 frames | `fps` 24, `duration` 0.25, `frames` 6, `has_alpha` false |
| `png_private20.mkv` | `png_rgba.mkv` with its `CodecPrivate` cut to its first 20 bytes, a `Void` element in the bytes it gave up | `fps` 24, `duration` 0.25, `frames` 6 |
| `mjpeg.mkv` | Motion JPEG (`V_MJPEG`), 3 frames | `fps` 24, `duration` 0.125, `frames` 3, `has_alpha` false |
| `block_track_zero.mkv` | `h264.mkv` with the track number of its first block starting with a 0 byte | refused: `not a Matroska or WebM file (a block at byte 523 is not valid)` |
| `truncated.mkv` | the first 1500 bytes of `h264.mkv` | refused: `not a Matroska or WebM file (an element at byte 40 runs past the end of the file)` |
| `garbage.webm` | the 11 bytes `not a video` | refused: `not a Matroska or WebM file (it does not start with an EBML header)` |

The fixtures cut from or written over another one (`truncated.*`, `mvhd_*`, `stsz_no_table.mp4`, `stsd_count_zero.mp4`, `png_private20.mkv`, `block_track_zero.mkv`) are made by `make_fixtures.py` from the bytes of the fixture named; but for `truncated.*`, every other size and offset stays where it was.

Generating the fixtures also reads each one the way stage-gen's engine did (`wave`, or `ffprobe` as §7 describes) and fails on any difference that §7 does not make deliberate. The deliberate ones among the fixtures: `empty.wav`, `rate_zero.wav`, `truncated_fmt.wav` and `odd_chunk_unpadded.wav` (it raised an error), `truncated.mp4`, `garbage.mp4`, `stsd_count_zero.mp4`, `mvhd_no_timescale.mp4`, `truncated.mkv`, `garbage.webm` and `block_track_zero.mkv` (it gave `bytes` and `kind`, or ffprobe read what was there), `stsz_no_table.mp4` (ffprobe read one packet), `vp9_5994.webm` (it gave 59.940063) and `png_rgba64.mkv` (it gave `has_alpha` false). Every other fixture reads the same in both, the rounded rates of `h264_ts1000_24fps.mp4`, `h264_ts1000_2997.mp4`, `h264_ts1000000_30fps.mp4`, `h264_ts90000_5994.mp4`, `ismv.mp4`, `fragmented_av.mp4` and `remux_ms.mp4` among them.

## 7. Changes from stage-gen's engine

stage-gen's engine computed facts in Python: images with Pillow, WAV with the `wave` module, and video with `ffprobe` when one was on `PATH`. Its video facts were the first video stream's `width`, `height`, `r_frame_rate` as `fps`, the stream's `duration` (else the container's), the demuxed packet count as `frames`, and `has_alpha` true when the decoder's pixel format was one of `rgba`, `bgra`, `argb`, `abgr`, `yuva420p`, `yuva422p`, `yuva444p`, `gbrap` and `ya8`. FX computes every fact itself, by the rules above.

| stage-gen's engine | FX |
|---|---|
| Video facts existed only where `ffprobe` was installed; without it, or when it failed or timed out, a video had `bytes` and `kind` only. | The engine parses MP4 and Matroska itself: the same facts everywhere ([identity.md](identity.md) §13). |
| An MP4 or Matroska file that does not parse had `bytes` and `kind` only (ffprobe failed), or what ffprobe could read from it. | Refused (§4.1, §5.1). |
| Images: 16-bit alpha judged on its high byte, `tRNS` ignored on 16-bit greyscale and RGB, a GIF's uncovered screen area counted, broken bytes crashed or dropped `opaque`. | §2's rule as written; broken bytes refused (§2's table). |
| WAV files that made `wave` raise anything but its own error stopped planning: an empty file (`EOFError`), a `fmt ` chunk cut short (`EOFError`), a chunk running past the RIFF size (`RuntimeError`), a frame rate of 0 (`ZeroDivisionError`). | No duration, never an error (§3). |
| Kinds came from the suffix, except that a workflow input with an unknown suffix took its declared kind: a `.mov` declared `video` had video facts, a `.bmp` declared `image` had image facts, and `audio/x-wav` and `audio` were read as WAV. | Kinds come from the suffix alone ([identity.md](identity.md) §4): a `.mov` or a `.bmp` is kind `file`, with `bytes` and `kind` only. |
| Facts were cached by digest alone, so the same bytes first read under one kind kept that kind's facts under another. | Facts depend on the bytes and the kind (§1). |
| `has_alpha` was membership of the decoder's pixel format in a list of nine 8-bit formats: 16-bit RGBA PNG frames (`rgba64be`) and ProRes 4444 had false, GIF frames in Matroska true. | By codec (§4.4, §5.3): PNG frames by their PNG header, so 16-bit RGBA is true; codecs the tables do not name have no `has_alpha`. |
| A Matroska frame rate was ffmpeg's reduction of 10⁹/`DefaultDuration` with numerator and denominator at most 30000 (19001/317, 59.940063, for 60000/1001), and an estimate when there was no `DefaultDuration`. | The closest fraction with a denominator at most 1001 (60000/1001, 59.94006); no `fps` without `DefaultDuration` (§5.2). |
| An MP4's frame rate and duration were ffmpeg's: its stream's `r_frame_rate`, an estimate for irregular timestamps, and its stream duration. | §4.3's rules, which give ffmpeg's values on every fixture, constant rates whose timestamps were rounded to a timescale of 1000, 16000, 90000, 10^6 or 10^7 units among them. ffmpeg may differ elsewhere: on a few frames at a coarse timescale, which allow both a whole rate and its 1000/1001 neighbour (ffmpeg then also reads the codec's own rate), on irregular timestamps, and on an `mdhd` duration that disagrees with the samples. |
| Numbers were Python floats, written `24.0`. | FX numbers: `24` (§1). |
