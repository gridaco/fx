# File-fact vectors

Media files that pin [facts.md](../../facts.md), one folder per container: `wav/`, `mp4/` and `matroska/`. §6 of facts.md says how each file was made and what it holds.

- **`<folder>/expected.json`** maps each file name in the folder to its facts, written as `facts(f)` gives them (`bytes`, `kind`, then the members facts.md gives the file, in that order), or to `{"refused": "<message>"}` for a file whose facts are refused. An implementation must compute exactly these values from the file's bytes and the kind of its suffix ([identity.md](../../identity.md) §4), member order included, and must refuse exactly the files marked refused. The text of a refusal after its prefix is informative (facts.md §1).
- **The files** were synthesized by [`tools/make_fixtures.py`](../../../tools/make_fixtures.py): WAV with Python's standard library, video with ffmpeg's `testsrc` and `sine` sources. Nothing here is generated art. The committed bytes are the vectors: another encoder build would write other bytes, so they are never regenerated to check them.

Check them with:

```sh
uv run --project python python tools/make_fixtures.py --check     # Python readers of facts.md, no binary
cargo test -p grida-fx-core --test facts_media                      # the engine
```
