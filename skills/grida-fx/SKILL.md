---
name: grida-fx
description: Author, plan, run, debug, and inspect Grida FX workflows using its installed CLI and SDKs. Use for FX workflow YAML, Python nodes and builders, JavaScript workflow authoring, routes, budgets, caching, run artifacts, and the local viewer.
---

# Grida FX

Use FX to describe work as a dependency graph with declared inputs and outputs.
The engine owns planning, scheduling, identities, caches, paid-call admission,
retries, and run records. AI generation is one application; ordinary local code
also runs as workflow steps.

Work in the user's project using installed packages. A repository clone, Rust,
Bun, Docker, and a frontend build are not prerequisites. Preserve existing project
structure and package-manager choices.

## Establish the installed interface

Prefer the user's existing installation. Check its version and command help
before relying on a feature:

```sh
grida-fx --version
grida-fx --help
```

Choose the installation appropriate to the project:

```sh
# Node 18+: project-local CLI and JavaScript/TypeScript SDK.
npm install @grida/fx@next
npx grida-fx --version

# Or invoke the published preview without adding a project dependency.
npx @grida/fx@next --version

# Python 3.11+: install into the project's virtual environment.
python -m pip install --pre 'grida>=0.1.0a1'
python -m grida.fx --version
```

These examples target the 0.1 preview. The Python version floor matters: the older
`grida 0.0.1` distribution is not FX. If a compatible release is unavailable for
the user's platform, report that rather than silently accepting an older package
or switching to a source build. Follow the project's lockfile when it pins FX.

The commands below use `grida-fx`; substitute `npx grida-fx`,
`npx @grida/fx@next`, or `python -m grida.fx` as appropriate. The Python package
does not install a `grida-fx` console command. npm's native engine is an optional
dependency: do not omit optional dependencies when installing it.

The preview targets macOS and Linux with glibc on x64/arm64; confirm a package is
available for the particular platform. Native Windows and Alpine/musl are not
supported. Windows users can use WSL 2. Python nodes, Python builders, and local
built-ins such as `image.resize`, `files.copy`, and `package` need Python with the
`grida` package even when invoked through npm. `GRIDA_FX_PYTHON` can select that
interpreter. Installed viewers carry their own web client.

## Author and plan

For new workflows or node bodies, read [authoring.md](references/authoring.md).
It includes runnable local examples and the supported Python/JavaScript paths.

1. Inspect the existing workflow and project settings. An `fx.yaml` is optional;
   add one when shared route defaults or a clear project home help. Keep generated
   `runs/` and `.fx/` data out of source control.
2. Discover the installed interface instead of guessing node fields or routes:
   `grida-fx nodes`, `grida-fx nodes image.generate`, and
   `grida-fx schema workflows/example.yaml`.
3. Connect steps through `${{ steps.NAME.outputs.OUTPUT }}`. These references
   establish dependencies. Use `needs:` only for ordering without data flow.
4. Plan with the intended inputs and inspect refusals before execution:

```sh
grida-fx plan workflows/example.yaml --inputs inputs/example.yaml --json
grida-fx plan workflows/example.yaml --inputs inputs/example.yaml --check
```

Planning admits no FX paid calls, but it may import Python modules, invoke a
builder, run declared local `at: plan` nodes, and cache results. Review unfamiliar
author code before loading it. Planning is not a sandbox or a promise of no writes.
Declared built-ins may be marked planned and lack an executable body or route;
do not present every listed type as runnable.

## Run within the user's intent

For local workflows, run without `--live`. This executes real local steps; it is
not a dry run. Existing paid results can replay from cache, while an uncached paid
call fails without live admission. For a workflow with paid steps, a supplied
stand-in can answer those calls offline:

```sh
grida-fx run workflows/example.yaml --inputs inputs/example.yaml --run runs/example
grida-fx run workflows/example.yaml --inputs inputs/example.yaml --stand-in tests/stand_in.py#answer
```

Stand-ins must implement FX's answer contract; they use a separate cache and
cannot combine with `--live` or `--yes-up-to`. See the
[stand-in guide](https://github.com/gridaco/fx/blob/main/docs/guide/05-running.md#stand-ins-testing-without-a-provider)
when authoring one.

A paid run requires the user's intent to make those calls and an agreed dollar
cap. Existing authorization remains valid within its scope. Inspect the plan's
routes and estimate, then pass the cap explicitly as `--live --max-usd N`, where
`N` is that agreed cap. A budget in a file or an estimate is not user authorization.
Use `grida-fx doctor workflows/example.yaml` to diagnose prerequisites; let FX
load credentials. Do not read or print `.env` or secret values, or embed them in
workflow files. Never add provider retries outside FX or bypass the ceiling with
direct provider calls.

Routes are `model@provider` choices for capabilities. Use the installed node
catalog and current provider documentation when choosing them. Project
`route_tables:` overlays the built-in table; explicit `--routes` files replace
the built-in table. Do not invent prices to make a refused plan pass.

## Inspect, resume, and deliver

```sh
grida-fx inspect runs/example --verify --json
grida-fx project runs/example
grida-fx view --run runs/example
```

Check exit status and recorded failures. SDK calls can return a failed run without
throwing: check `result.ok` and `result.incomplete`, then failures/stopped state.
Resume with the same target, inputs, and `--run` folder; changed plans may be
refused. Preserve records while diagnosing failures. Do not clear caches or
forget provider jobs merely to force another attempt.

To place results in the user's destination, use `--deliver output=path` during
`run`, or the SDK's file-copy/delivery APIs after a successful result. Store files
are immutable: copy them out rather than editing cache paths. A new run folder
does not force fresh generation. For intentional alternatives, inspect the
installed `reroll`, `pick`, and `takes` help; a reroll can spend and needs the same
authorization discipline as a run.

Report the actual run folder, completed/failed/incomplete state, delivered files,
and FX's recorded cost. Distinguish an estimate, engine-booked cost, and provider
billing. Detailed provider usage is not currently retained, and the open image
billing discrepancy has no confirmed cause. See the
[current issues](https://github.com/gridaco/fx/blob/main/ISSUES.md) when diagnosing
cost or identity behavior; do not claim those gaps are fixed.

## View a plan or run

```sh
grida-fx view workflows/example.yaml --inputs inputs/example.yaml
grida-fx view assets.py:build --arg theme=dusk
grida-fx expand workflows/example.yaml --inputs inputs/example.yaml > graph.json
grida-fx view --plan graph.json
grida-fx view --run runs/example --no-open
```

Each `view` starts its own foreground loopback server and normally opens a browser.
Ctrl-C stops that viewer; closing the tab does not. `--no-open` prints the URL
without launching a browser; `--port N` requests a fixed local port. No separate
web setup is required.

The canvas is read-only, with pan/zoom, named ports, imported-workflow navigation,
and recorded artifact previews. Run updates require **Refresh**. Plan views are
static for the session. `run` does not automatically launch the viewer, and there
is no live stream or shared background server. Custom HTML views and preview
callbacks are not rendered yet. Saved plan/run viewing loads no author code;
viewing a source target invokes the planner. Missing or cache-only artifacts may
be unavailable in a copied run; viewing never reruns steps to recover them.
