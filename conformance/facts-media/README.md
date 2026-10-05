# facts-media

`in/voice.wav` and `in/clip.mp4` are copies of two file-fact vectors, `spec/vectors/facts/wav/pcm16_8k.wav` and `spec/vectors/facts/mp4/h264_2997.mp4`. [`tools/make_fixtures.py`](../../tools/make_fixtures.py) made both and keeps these copies equal to them (`--check` compares the bytes); [spec/facts.md](../../spec/facts.md) §6 says how:

- `voice.wav` (2044 bytes) was written by Python's `wave` module: 1000 frames of silence, mono, 16-bit, at 8000 Hz. Its facts are `{"bytes": 2044, "kind": "audio/wav", "duration": 0.125}`.
- `clip.mp4` (2835 bytes) is 30 frames of ffmpeg's `testsrc` picture at 64×48 and 30000/1001 frames per second, encoded by x264 and written with ffmpeg's bit-exact flags. Its facts are `{"bytes": 2835, "kind": "video/mp4", "width": 64, "height": 48, "fps": 29.97003, "duration": 1.001, "frames": 30, "has_alpha": false}`.

The committed bytes are the fixtures; another encoder build would write other bytes and a different digest.

## What the case pins

- File facts of a WAV and an MP4 input are read while planning, and enter identities only through the values they render into (`identity.json`).
- `describe` is planned: its `if:` compares the clip's `fps` (29.97003, above 24) and the voice's `duration` (0.125) with the clip's `duration` (1.001).
- Its `manifest`, a value of the free built-in `fx/package@1`, holds `seconds`, `fps`, `frames` and `width` as numbers (`0.125`, `29.97003`, `30`, `64`; whole numbers without `.0`), and its `label` renders `64x48, 30 frames at 29.97003 fps for 1.001 s; voice 0.125 s, 2044 bytes of audio/wav`, each number in its JCS form.
- `long_clip` is absent: the clip has 30 frames and no alpha.
- The case has no `nodes/` folder: planning it needs no Python.
