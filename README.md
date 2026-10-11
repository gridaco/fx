# Grida FX

**Go with the flow.**

Reusable workflows for images, audio, and more. Connect your code and AI models,
bring your own provider keys, and turn a one-off script into something you can run again.

- **See the plan first.** Inspect the steps, dependencies, and estimated cost before a paid run.
- **Keep the work you already did.** Reuse matching results from the cache and resume interrupted runs.
- **Look inside a run.** Open the node canvas to explore steps, connections, and recorded artifacts.
- **Test without a bill.** Run ordinary code offline, or use [stand-ins](docs/guide/05-running.md#stand-ins-testing-without-a-provider)
  to answer paid calls in tests.

## Install

FX comes with the Grida CLI:

```sh
npm install -g grida
grida fx --version
```

The CLI needs Node.js 24 or later and carries the FX version it was released with, so updating
`grida` updates FX. FX runs on macOS, and on Linux (x64 and arm64) with glibc 2.28 or later; on
Windows, use WSL 2.

Write custom nodes and builders in Python with the Python SDK:

```sh
pip install grida
```

The JavaScript SDK is `@grida/fx`. To run FX without the Grida CLI (in a service you host, say),
see [ways to run FX](CONTRIBUTING.md#ways-to-run-fx).

## Plan. Run. Inspect.

FX keeps a project dashboard and records every run. For an existing workflow, start the
service once:

```sh
grida fx init
grida fx start --background
grida fx plan workflows/example.yaml --open
grida fx run workflows/example.yaml --run runs/example --open
grida fx inspect runs/example --open
```

A run without `--live` executes local steps and reuses cached paid results.
New provider calls require `--live` and a spending ceiling, such as `--max-usd 5`.
The dashboard stays at `http://127.0.0.1:8787/` by default after execution finishes;
`grida fx stop` stops the service. Browser opening requires `--open`.
The [viewing guide](docs/guide/07-viewing.md) covers independent `--standalone`
inspection and the older `view` command.
To share a finished recording, use
`grida fx export runs/example --out review --json`; the
[sharing guide](docs/guide/11-exports.md) explains disclosure and portable media.
To stop execution, use `grida fx cancel runs/example --wait`. The
[run-control guide](docs/guide/08-run-control.md) explains targeting, cleanup and resume.

Start with [the guide](docs/guide/01-getting-started.md), or explore the
[examples](examples/): recolor an image with code, build a looping background,
or compose a concept gallery.

### For agents

Install the skill for workflow authoring, execution, and inspection:

```sh
npx skills add gridaco/fx
```

## Under the hood

FX is a workflow engine for asset pipelines, implemented in Rust. YAML declares
the dependency graph; the Python and JavaScript SDKs provide programmatic authoring
and execution APIs. Python also hosts custom node bodies and workflow builders.

The engine owns planning, scheduling, result identities, caching, spending limits,
provider retries, and run records. SDKs and node bodies use that engine rather than
implementing their own execution or billing layer. AI generation is one application;
ordinary code can run as workflow steps too. The bundled viewer is a read-only
canvas for plans and recorded runs.

Read the [workflow language](docs/guide/02-workflow-file.md),
[Python SDK](python/README.md), [JavaScript SDK](js/fx/README.md),
and [specifications](spec/) for the technical contracts.

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) for source builds, examples, checks, and
viewer or website development. [The overview](docs/wg/overview.md) records the
project's history and direction; [ISSUES.md](ISSUES.md) tracks known gaps.

Grida FX is open source under [Apache-2.0](LICENSE).
