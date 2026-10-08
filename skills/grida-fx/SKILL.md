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
npm install @grida/fx
npx grida-fx --version

# Or invoke the stable release without adding a project dependency.
npx @grida/fx --version

# Python 3.11+: install into the project's virtual environment.
python -m pip install 'grida>=0.1.0'
python -m grida.fx --version
```

These examples target FX 0.1. The Python version floor matters: the older
`grida 0.0.1` distribution is not FX. If a compatible release is unavailable for
the user's platform, report that rather than silently accepting an older package
or switching to a source build. Follow the project's lockfile when it pins FX.

The commands below use `grida-fx`; substitute `npx grida-fx`,
`npx @grida/fx`, or `python -m grida.fx` as appropriate. The Python package
does not install a `grida-fx` console command. npm's native engine is an optional
dependency: do not omit optional dependencies when installing it.

FX supports macOS and Linux with glibc on x64/arm64; confirm a package is
available for the particular platform. Native Windows and Alpine/musl are not
supported. Windows users can use WSL 2. Python nodes, Python builders, and local
built-ins such as `image.resize`, `files.copy`, and `package` need Python with the
`grida` package even when invoked through npm. `GRIDA_FX_PYTHON` can select that
interpreter. Installed viewers carry their own web client.

## Author and plan

For new workflows or node bodies, read [authoring.md](references/authoring.md).
It includes runnable local examples and the supported Python/JavaScript paths.

1. Inspect the existing workflow and project settings. An `fx.yaml` is optional;
   the recommended setup is `grida-fx init` when the installed help lists it. It
   preserves existing configuration and creates no workflow or service. Keep
   generated `runs/` and `.fx/` data out of source control.
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
```

Check exit status and recorded failures. SDK calls can return a failed run without
throwing: check `result.ok` and `result.incomplete`, then failures/stopped state.
Current source supports `run TARGET --name NAME` (create-only) and
`run TARGET --resume NAME` (requires existing run); confirm installed help before
using them. Without a name or folder every command creates a fresh run record,
even when cached. Names are scoped to recorded workflow ID/source within a project;
do not overwrite or infer a new run from the last resumed time.
Resume with the same target, inputs, routes, takes and execution mode, using
`--resume NAME` or the same `--run` folder; a changed plan is refused. Prior arguments
are not restored automatically. Put overlapping workflow inputs after `--`, for
example `run greeting --name baseline -- --name Ada`, or use an inputs file.
Inspect `WORKFLOW_ID/NAME` for a precise named run, or an actual folder; a workflow
ID selects newest-created evidence and refuses ambiguous recorded sources.
Preserve records while diagnosing failures. Do not clear caches or
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

## Project service and browser inspection

Check `grida-fx --help` first. The service below is current source behavior; an
older installed release may lack `init` and `start`. Do not assume a stable
package contains every feature described by the main branch.

When these commands are available, normal project work uses:

```sh
grida-fx init
grida-fx start --background --json
grida-fx status --json
grida-fx plan workflows/example.yaml --inputs inputs/example.yaml --open
grida-fx run workflows/example.yaml --inputs inputs/example.yaml --run runs/example --open
grida-fx inspect runs/example --open
```

Treat this as a sequence of tools to select from, not permission to execute a
workflow. `init` creates only minimal `fx.yaml` configuration and never overwrites
it. `start --background` returns after readiness and reuses a compatible service
for that project. Plain `start` stays foreground. The default address is
`http://127.0.0.1:8787/`; use the actual URL from `status --json` or command output.
An explicit `--port N` must be available and is remembered for that project.

Commands never start a background service implicitly. With an explicit project,
`run` still executes if the service is absent; `--open` reports how to start it.
`plan --open` and `inspect --open` require the service too. Do not mix browser
flags with JSON plan/inspect output. A run prints an available URL without
`--open`; omit that flag when the person does not want a browser launch.
The service loads no author code, but planning a source target can do so.

A run reports its actual usable URL after initialization and before run-phase
steps. Relay it promptly while continuing to supervise the run. Planning-time
code precedes that URL. The page remains available after execution finishes,
including after a service restart while the record is retained. Run pages follow
recorded events about once a second; saved plan pages remain static.

`grida-fx logs --lines 50` reads service logs. `grida-fx stop` stops inspection,
not workflow execution, and preserves data. Service commands accept
`--project DIRECTORY`. Background mode is detached from the terminal; it does
not install automatic crash restart or login/boot activation.

For probes and independent inspection, use `--standalone` even inside a project:

```sh
grida-fx run workflows/example.yaml --standalone --open
grida-fx plan workflows/example.yaml --standalone --open
grida-fx inspect runs/example --standalone --open
```

No `init` or `start` is required. A standalone run's server uses an available
port and ends with execution; standalone plan/saved-run inspection stays foreground
until interrupted. Projectless runs use this mode by default, as do projectless
`plan --open` and `inspect --open`. It does not clear caches or force fresh work.
`--no-view` skips run viewer integration and conflicts with `--open`; SDK runs
use it and return results after execution without a live viewer handle.

The hidden `view` command remains for compatibility, including saved graph files:
`grida-fx view --plan graph.json --open`. In older installations, use the supported
`view TARGET` or `view --run RUN` interface. Check that command's help: earlier
versions open by default, whereas versions listing `--open` opt in; `--no-open`
is accepted for compatibility and conflicts with `--open`.

The canvas supports pan/zoom, named ports, imported-workflow navigation, and
recorded artifact previews. Custom HTML and preview callbacks are not rendered.
Missing artifacts remain unavailable; inspection never reruns steps to recover
them. Use the printed run URL rather than reconstructing opaque project/run IDs.

When `observe` is available, `observe RUN --snapshot` attaches to a consistent
recorded prefix. Continue with `observe RUN --after CURSOR --limit 256`; apply a
batch before saving its returned cursor and drain while `has_more`. A terminal
event ends one invocation; a later resume can append more. Unfinished state does
not prove process liveness, and a stopped viewer does not prove a stopped workflow.
