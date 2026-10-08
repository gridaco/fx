# Example: image-recolor

Read an image, recolor it with a small algorithm, and wait five seconds before returning the
result. These are three ordinary Python nodes: Pillow rotates `(red, green, blue)` to
`(blue, red, green)` while preserving alpha, and `time.sleep` supplies the delay. Everything
runs offline at $0; no provider, route, key, or stand-in is needed.

Each node has its own source file under `nodes/`, and
[`workflows/image-recolor.yaml`](workflows/image-recolor.yaml) connects them.

Use a Python with `grida` and Pillow installed, and an FX engine with its embedded viewer
([From a clone](../README.md#from-a-clone)). From this folder:

```bash
python make_input.py
grida-fx init
grida-fx start --background
grida-fx plan image-recolor --inputs inputs/example.yaml
grida-fx plan image-recolor --inputs inputs/example.yaml --open
```

These service commands describe current source. Check installed `--help`; older
releases use the compatibility viewer described in the [viewing guide](../../docs/guide/07-viewing.md#standalone-inspection).

The plan prints:

```text
image-recolor  ·  1 phase
phase 1   3 steps   0 provider calls   $0.00
cached    0 of 1 known steps
estimate  $0.00
```

The plan canvas shows the graph without running the nodes. Keep the service running,
then execute and inspect the recorded inputs, intermediate images, final images, and durations:

```bash
grida-fx run image-recolor --inputs inputs/example.yaml --run runs/first --open
grida-fx inspect runs/first --open
```

`runs/first/outputs/before.png` is the normalized input and `after.png` is the recolored
image. The final node returns the same image after its delay, so the recolor and delay
outputs have the same digest. Each step's placed image also lives under `runs/first/files/`.

`run --open` opens its page before run-phase execution. The page follows the five-second
wait automatically and remains available after the command exits. A plan page stays static.
Use `grida-fx stop` when finished inspecting; stopping the service does not stop a workflow.

Run again with `--run runs/second`: all three nodes use the cache, including the delay, so
there is no second five-second wait. A new run folder alone does not bypass the cache.
Change `wait_seconds` in `inputs/example.yaml` to rerun only the delay, or use a fresh copy of
the example without its `.fx/` folder for a fully uncached run. `wait_seconds: 0` skips the wait.

The fixture is original, code-drawn media. [`make_input.py`](make_input.py) is its complete
provenance and recreates the same pixels; the generated `inputs/source.png` stays outside Git.
You can replace that file with another image or change the input YAML to a path you own.
