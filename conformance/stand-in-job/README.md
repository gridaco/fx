# stand-in-job

A stand-in answers two long jobs, `video.generate` and `mesh.generate`, at once: a stand-in run
reads and writes no job record ([protocol.md](../../spec/protocol.md) §6.1, *Stand-in answers*;
[store.md](../../spec/store.md) §8, *Stand-in runs*). The routes are made up (`clip-a@acme`,
`mesh-a@acme`): FX has no adapter for them, so each answer is held only to its capability's shape
and every-route check ([capabilities.md](../../spec/capabilities.md) §1, §6, §7).

## The files

- `in/clip.mp4` (2835 bytes, sha256
  `dc6118be05f87450cb663b9d42c66cb16d0e5fe2f5114c918c972db38fda589d`) is a copy of the file-fact
  vector `spec/vectors/facts/mp4/h264_2997.mp4`, the same bytes as `facts-media/in/clip.mp4`.
  [`tools/make_fixtures.py`](../../tools/make_fixtures.py) made it with ffmpeg 9.0.1, as
  [spec/facts.md](../../spec/facts.md) §6 says: 30 frames of `testsrc` at 64×48 and 30000/1001
  frames per second, H.264 by x264 in yuv420p without its version message, written bit-exact:

  ```sh
  ffmpeg -hide_banner -loglevel error -nostdin -y \
    -f lavfi -i testsrc=size=64x48:rate=30000/1001 -frames:v 30 \
    -c:v libx264 -pix_fmt yuv420p -bsf:v filter_units=remove_types=6 \
    -fflags +bitexact -flags:v +bitexact -flags:a +bitexact -map_metadata -1 h264_2997.mp4
  ```

  Its facts, from `spec/vectors/facts/mp4/expected.json`, are `{"bytes": 2835, "kind":
  "video/mp4", "width": 64, "height": 48, "fps": 29.97003, "duration": 1.001, "frames": 30,
  "has_alpha": false}`. The committed bytes are the fixture; another encoder build would write
  other bytes. `tools/make_fixtures.py` writes this copy with the vector, and its `--check` fails
  when the copy is not the vector's bytes.
- The front view and the model are made by `in/stand_in.py` when it is asked, never committed:
  the view is a 64×64 RGB PNG of one colour, its image data zlib with stored deflate blocks (the
  same bytes whatever zlib Python has); the model is a 48-byte binary glTF 2.0 file, its 12-byte
  header (`glTF`, version 2, length 48) and one JSON chunk of 28 bytes,
  `{"asset":{"version":"2.0"}}` padded with one space.

## What the case pins

- The stand-in answers `null` data, and the engine writes it from the files: the clip's
  `{"facts": {"width": 64, "height": 48, "duration_seconds": 1.001, "fps": 29.97003}}` and the
  model's `{"facts": {"model_kind": "model/gltf-binary"}}`. A paid built-in makes each member of
  `data.facts` a node fact, so they are `clip#1`'s and `mesh#1`'s node facts in
  `events-one.json` and `project-one.json`, beside `cost_usd: 0`.
- Files sent without a kind get the capability's: `video/mp4` for `video`, `image/png` for
  `image`, and for `model` the kind its bytes show.
- No `jobs/` folder exists in the project's store or in the stand-in store, and `grida-fx jobs`
  prints nothing (`jobs.txt` is empty).
- The mesh's request carries its view as `{"views": {"front": {"file": <digest>}}}`, and the
  stand-in is handed that file's ref with its facts (`asked.json`).
