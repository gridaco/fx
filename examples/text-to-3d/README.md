# Example: text to 3D

**The ask:** "Turn my description into one reference image, then build a textured 3D asset from
that image. Let me inspect both results."

```text
text description → reference image → textured model (Tripo P2)
```

[`asset.py`](asset.py) exposes `generate_3d_asset(text: str) -> Workflow`. It describes exactly
two engine-managed steps. Calling the builder only constructs a workflow; planning does not
spend. The engine owns execution, pricing, cache identities and provider-job recovery.

Tripo P2 is the model generator this example demonstrates. The reference image can come from
another compatible image route; the workflow itself requests `image.generate` without naming
an image provider. Tripo receives that generated reference as its single-image input. The mesh
requests 5,000 triangular faces (`quad: false`), textures and PBR materials. This is a mesh
example: it does not rig the asset or generate animation.

## Install and configure

Copy this folder anywhere. It needs Python 3.11 or later and the current FX engine/SDK:

```sh
npm install -g grida
uv sync
```

Use the Python environment for FX's builder host. From this folder:

```sh
export GRIDA_FX_PYTHON="$PWD/.venv/bin/python"
```

The supplied [`fx.yaml`](fx.yaml) chooses `openai/gpt-image-2.5-sunburst` for the reference
image, with its provider preference set separately to `openrouter`. Tripo P2 is explicit because
it is the model generator this example demonstrates:

```yaml
routes:
  image.generate: openai/gpt-image-2.5-sunburst
  mesh.generate: P2-20260801@tripo
provider_preferences:
  image.generate: openrouter
```

OpenRouter was used for live verification because that credential was available; it is not
a requirement of the image-to-3D pipeline. With an OpenAI key, change only
`provider_preferences.image.generate` to `openai`; the model and workflow stay unchanged.
FX resolves the canonical model to that provider's native route before pricing and execution.
It does not choose from available keys or try another provider after a failure. Existing
qualified `model@provider` selections remain exact overrides. Re-plan after changing the model
or provider: prices and cache identities differ. The estimate below uses OpenRouter.

With the supplied configuration, set `OPENROUTER_API_KEY` and `TRIPO_API_KEY` in your shell or
in a local `.env`; the OpenAI alternative uses `OPENAI_API_KEY` and `TRIPO_API_KEY`. Keep `.env`,
generated artwork, run records and caches out of version control. FX reads credentials through
its allowlisted environment loader; this example neither reads nor writes keys. See
[AI providers](https://github.com/gridaco/fx/blob/main/docs/guide/13-providers.md).

## Plan, run and preview

These commands use the authored default: a small moss-green ceramic lantern with a rounded body,
a wide handle and three amber window openings.

```sh
grida fx init
grida fx start --background
grida fx plan asset.py:generate_3d_asset
```

The engine prints:

```text
text-to-3d  ·  1 phase
phase 1   2 steps   2 provider calls   $1.41 – $3.00
cached    0 of 1 known steps
estimate  $1.41 – $3.00
```

Review that estimate, then authorize a paid run with a total ceiling:

```sh
grida fx run asset.py:generate_3d_asset --live --max-usd 3 --name lantern --open
```

Select **Reference image** to inspect the generated PNG; select **Textured 3D model** to rotate
and zoom the generated model. Triangular P2 output is normally GLB, which the viewer previews
inline with its embedded materials. A provider-returned format the viewer cannot preview keeps
its original-file link. No Blender, conversion step or separate 3D application is required for
a supported GLB. See
[3D previews](https://github.com/gridaco/fx/blob/main/docs/guide/07-viewing.md#inspect-3d-results).

The root outputs are `reference` and `model`, placed in the run folder's `outputs/` with their
recorded media suffixes. Reopen the saved run without generation:

```sh
grida fx inspect text-to-3d/lantern --open
```

To use your own description, pass the same argument to planning and execution:

```sh
grida fx plan asset.py:generate_3d_asset --arg 'text=A squat blue ceramic birdhouse with a round entrance and a broad roof.'
grida fx run asset.py:generate_3d_asset --arg 'text=A squat blue ceramic birdhouse with a round entrance and a broad roof.' --live --max-usd 3 --name birdhouse --open
```

A ceiling limits spending; it does not guarantee completion or artistic quality. Repeating the
same request can reuse both cached outputs. Changing the description invalidates the reference;
the mesh's identity follows the actual image bytes. Use `--resume lantern` with the same original
arguments to continue the named run after an interruption, preserving its record and provider
job. Do not create a fresh paid run to recover an accepted task. See
[Running](https://github.com/gridaco/fx/blob/main/docs/guide/05-running.md).

## Use from Python

From this folder, the SDK calls the same public builder target and keeps both outputs:

```python
from grida.fx import plan, run

request = {"text": "A squat blue ceramic birdhouse with a round entrance and a broad roof."}
planned = plan("asset.py:generate_3d_asset", arguments=request, max_usd=3)
print(planned.estimate())  # review before authorizing live=True
result = run(planned, live=True, name="birdhouse")
if not result.ok:
    raise RuntimeError(result.failures)
print(result.cost, result.outputs["reference"].path, result.outputs["model"].path)
```

`generate_3d_asset(text).document()` is useful for inspecting the authored graph. To execute it,
pass the builder target and `arguments` to the SDK, as above, rather than passing a `Workflow`
object. The CLI and SDK share the same engine contracts. See
[Python SDK](https://github.com/gridaco/fx/blob/main/python/README.md).

## Live verification

Live verification on 2026-11-12 used the default lantern description and the concrete routes
`openai/gpt-image-2.5-sunburst@openrouter` and `P2-20260801@tripo`, with an approved $3 ceiling.
OpenRouter produced a 1024×1024 PNG and Tripo P2 produced a
3,061,812-byte GLB with one mesh, one material and three embedded texture images. FX recorded
$0.42244 for the reference and $1.10 for the mesh, $1.52244 total. Both saved files passed
`inspect --verify`; repeating the same builder with the network disabled reused both steps at
$0. These are one run's recorded charges, not a fixed price or a quality guarantee. Generated
artwork, keys and saved runs stay outside Git.

## Check the wiring without providers

[`stand_in.py`](stand_in.py) synthesizes its own PNG lantern silhouette and a simple GLB cuboid.
Its mesh answer checks that it received that exact reference image. These are structural test
fixtures, not examples of generated asset quality. They require no keys or network access:

```sh
grida fx run asset.py:generate_3d_asset --stand-in stand_in.py#answer --name offline --open
```

Stand-in results use a separate cache from paid results. Without a stand-in and without
`--live`, the reference step is refused at $0 and no mesh is submitted. CI exercises this
refusal and the stand-in chain from fresh copies of the example. Nothing generated is committed.
