# Viewing workflows and runs

The local FX service keeps a project's plans and recorded runs available at one
loopback address. Its bundled viewer displays each workflow on a read-only canvas.
These commands describe current source behavior; check your installed version's
`--help` because older published packages may not include the service yet.

## Start the project service

From your project directory:

```sh
grida-fx init
grida-fx start --background --open
grida-fx status --json
```

`init` preserves an existing `fx.yaml` or creates a minimal one. The dashboard is
at `http://127.0.0.1:8787/` by default, with one row per recorded workflow.
Use `start --port N` to choose another exact port; an occupied port is an error,
not a reason to silently pick a random port. FX remembers a successful selection.
`status --json` reports the actual address and project/service identity.

Omit `--background` to keep the service in your terminal. Background startup
returns only after readiness and reuses a compatible service for the same project.
It survives the launching command and terminal, but does not install automatic
crash restart or login/boot activation. No Docker or frontend server is needed.

```sh
grida-fx logs --lines 50
grida-fx stop
```

`stop` closes the selected project's service without deleting records or stopping
workflow processes. Restarting it restores access to retained run URLs. Closing a
browser tab has no effect on service or execution. Service commands accept
`--project DIRECTORY` when you are outside the project.

## Use the canvas

Scroll with two fingers or drag the background to pan. Pinch to zoom around the
pointer, fit the graph to the window, and select a node to inspect it. Ctrl+wheel
also zooms; ordinary wheel scrolling pans.
Nodes and connections cannot be edited. The step list provides the same selection
without using the canvas.
Running steps have a blue card outline and tint, an activity indicator, and a
highlighted row in the step list. The highlight follows recorded state updates
without changing your selection or camera. Reduced-motion settings disable the indicator pulse.
The viewer keeps a desktop layout: the inspector hides below 1280 px and the step
sidebar below 1024 px. The canvas fills the remaining window; sidebars never stack.
Connections stay muted until you hover a wire or a node; hovering a node highlights
all its incoming and outgoing connections.

Cards show the node's named, typed input and output ports. A list or keyed collection
uses one port. Solid wires join the particular output and input referenced by the
workflow; several wires can connect the same two steps through different ports.
Parameters that read another node have a distinct parameter socket, and referenced
node facts have fact sockets. Literal parameters remain settings. Ordering, judging,
and other dependencies use dashed lines between whole cards.

The engine records this connection metadata. Expressions can combine several sources;
their wires show those references, not an assertion that a parameter equals one
upstream value. Unresolved choices retain dependency lines until a run resolves them.
Older saved plans and runs remain viewable with dependency lines when they contain
no port metadata; the viewer never guesses connections from matching artifacts.

## Enter an imported workflow

A step that imports another workflow appears as one card with its named inputs
and outputs. Choose **Open workflow**, or double-click the card, to inspect its
internal steps. Use **Back**, a breadcrumb, or Escape while the canvas is focused
to return to an enclosing workflow. Repeated imports have separate cards and
navigation paths, even when they use the same workflow file.
Returning restores that workflow occurrence's canvas position, zoom and selection.
Refresh preserves your location in the workflow hierarchy.

Boundary ports keep the imported workflow's public names. Renamed outputs and two
aliases of the same internal result remain distinct connections. Inline `steps:`
groups stay on the current canvas inside labeled frames.

Enclosing cards show internal step and failure counts. A failed child remains
visible through its enclosing workflow cards; selecting one exposes the recorded
errors before you enter it. Older saved records without scope metadata keep their
flat canvas instead of guessing workflow boundaries from step names.
Recorded pending repeats remain unresolved inside their enclosing workflow. A
declared alias without an observed source has no invented named wire; known step
dependencies remain dashed connections.

## Open a workflow plan

With the project service running, name the same target you would execute:

```sh
grida-fx plan workflows/gallery.yaml --inputs inputs.yaml --open
grida-fx plan gallery --caption "A copper lantern" --open
grida-fx plan assets.py:build --arg theme=dusk --open
```

FX plans once, saves the resulting `fx-graph-v1`, and opens its project page.
It shows dependencies, parameters, routes, identities, estimates and planning
problems. Conditional and absent steps remain visible; values that depend on
execution remain unresolved. Planning state `done` may describe an existing or
planning-time result and does not claim a new run succeeded.

Planning admits no FX paid calls or run-phase steps. It may import Python builders
and node modules, execute declared local `at: plan` steps, and cache their results.
Python builders use the existing `file.py:function` contract; arbitrary execution
scripts are not loaded to infer workflows. JavaScript-authored YAML files work
like other files; a direct JavaScript builder host remains later work.

Plan pages are static materializations. They do not poll execution events or
replan when source changes. Browser-serving flags cannot be mixed with `--json`.

## Open a recorded run

```sh
grida-fx inspect runs/example --open
grida-fx inspect example --open
grida-fx inspect example/baseline --open
```

This registers and opens the selected record in the running project service.
It reads recorded states, inputs, outputs, failures, costs and available artifacts;
it never imports author code or calls a provider. The Python distribution uses
exactly the same interface: `python -m grida.fx inspect runs/example --open`.
For machine-readable inspection, use `inspect runs/example --verify --json`
without browser flags.

## Watch execution

```sh
grida-fx start --background
grida-fx run workflows/example.yaml --run runs/example --open
```

After planning and run initialization, `run` reports its usable project run URL
before run-phase steps execute. Report that actual URL to a person instead of
constructing it from a folder name. `--open` requests one browser launch; omit it
to visit the printed URL yourself. Planning-time code can execute before the URL
exists. Browser or service failures do not change the workflow's result.

`run` stays foreground and exits normally after execution. The separate FX service
keeps its page available afterward, including for failed and cancelled runs.
The URL still identifies that record after a service restart or same-plan resume.
The dashboard opens each workflow's newest-created run, including a failed run.
Expand its history for earlier names, creation times and states, or a saved plan.
Older unfinished runs stay visible even when a newer run exists. **Project** returns
from a canvas to that index. Resume retains the original creation time and URL.
Grouping uses the recorded workflow ID and source, so changed inputs or definitions
at that path stay together; distinct sources sharing an ID remain separate.
Old records without sufficient source metadata remain separate rather than guessed.

With an explicit project, `run` registers its record even when the service is
stopped. Execution proceeds, and `--open` reports how to start the service.
`plan --open` and `inspect --open` require that project's running service as well.
None of these commands starts a background process implicitly.

Run views follow the public [observation contract](../../spec/observation.md):

```sh
grida-fx observe runs/example --snapshot
grida-fx observe runs/example --after CURSOR --limit 256
```

Apply a batch before saving its returned opaque cursor. A terminal event ends
one invocation; a resumed invocation can append more events to the same folder.
Unfinished recorded state does not prove the execution process is alive.
Python/JavaScript SDK runs suppress viewer integration and return results after
execution; they do not provide a live browser or observation handle.

## Standalone inspection

For tests, probes and one-off work, retain an independent viewer:

```sh
grida-fx run workflows/example.yaml --standalone --open
grida-fx plan workflows/example.yaml --standalone --open
grida-fx inspect runs/example --standalone --open
```

No `init` or `start` is needed. This explicit mode works inside an existing
project too. It changes viewer ownership and lifetime, not workflow resolution,
budgets, record formats or cache reuse. Without an `fx.yaml`, `run` uses standalone
viewing by default; projectless `plan --open` and `inspect --open` do likewise.

The independent server uses an available loopback port. A standalone run's server
ends when execution returns, fails or is cancelled. Standalone plan and saved-run
inspection stays foreground until Ctrl-C. Omit `--open` to print the URL without
launching a browser. `run --no-view` skips viewer integration and conflicts with
`--open`; SDK run methods use it.

The old `view` command remains hidden compatibility syntax for independent source,
saved-plan and run viewing. It still supports `--open`, an exact `--port N`, and
`--no-open` for older scripts. To inspect an already materialized graph without
loading author code:

```sh
grida-fx expand workflows/gallery.yaml --inputs inputs.yaml > graph.json
grida-fx view --plan graph.json --open
```

## Run records and artifacts

The selected directory supplies `plan.json`, `events.jsonl`, and placed output
files. YAML and Python-authored runs use these same recorded contracts; the viewer
does not need to import their workflow modules. A stand-in run is visibly marked.

Select a step to inspect its recorded values and artifacts. The viewer follows the record automatically, polling about once a second.
Refresh also reattaches to the latest consistent snapshot. Images, text/JSON,
audio, and video use generic previews; other file types can be opened in a new tab.

Run canvas cards also show the first available recorded image output. A count
indicates multiple distinct available images; select the card to inspect all outputs.
The canvas uses the verified artifact inventory and never evaluates author code to
produce thumbnails. Plan cards have no artifact previews, including steps marked
`done` by the planner. Updates preserve the canvas camera, selection and workflow navigation.

These automatic thumbnails use structured run data. The existing authoring `view`
declarations are booleans or HTML template paths; custom templates and structured
preview declarations or callbacks are not yet rendered by this viewer.

Files are matched to recorded content digests. A missing, changed, or store-only file
is unavailable; this bootstrap does not discover the originating project's cache.
Consequently, a copied run may show its placed outputs while some inputs are absent.
HTML and other active documents are not executed as viewer pages.

Recorded spend is what the engine booked. It is distinct from a plan estimate and
does not imply that detailed provider usage was retained. Absent cost evidence is
shown as unknown. The viewer does not change the source run or recover missing files
by rerunning anything.

## Distribution and later work

The viewer's production JavaScript and CSS are embedded in the native engine.
Installed users need no frontend dev server, frontend build, Docker, or internet
connection to view a local run. The npm and Python launchers retain their normal
runtime requirements. Contributor builds are described in [CONTRIBUTING.md](../../CONTRIBUTING.md#build-from-source).

Contributors can use the [canonical viewer fixtures](../../fixtures/viewer/README.md)
to generate and check real provider-free plans/runs covering branches, matrices,
repeats, conditions, media, cache, failure, and takes. Generated data stays ignored;
the fixture's expectations are the regression baseline.

Custom node views, portable web snapshots, machine-wide project aggregation and
OS-managed startup/recovery remain later work. The [service contract](../../spec/service.md)
owns current lifecycle guarantees; [TODO](../../TODO.md) records open ideas.

## Cancellation intent

The run sidebar shows `cancelling` after a recorded `cancel_requested` event and
returns to the terminal state when that invocation ends. A resume resets the intent.
This label does not prove the runner is still alive or that cleanup finished. Use
[`inspect --control` or `cancel --wait`](08-run-control.md) for verified local control.
The viewer has no cancellation button or write endpoint in this version.
