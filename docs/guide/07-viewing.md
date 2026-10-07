# Viewing workflows and runs

The local viewer shows a workflow plan or one existing FX run on a node canvas.
Scroll with two fingers or drag the background to pan. Pinch to zoom around the
pointer, fit the graph to the window, and select a node to inspect it. Ctrl+wheel
also zooms; ordinary wheel scrolling pans.
Nodes and connections cannot be edited. The step list provides the same selection
without using the canvas.
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

Name the same target you would give to `plan` or `run`:

```sh
grida-fx view workflows/gallery.yaml --inputs inputs.yaml
grida-fx view gallery --caption "A copper lantern"
grida-fx view assets.py:build --arg theme=dusk
```

The viewer uses FX's existing offline planner once, then serves the resulting
`fx-graph-v1` document. It shows dependencies, parameters, routes, step identities,
estimates and planning problems. Conditional and absent steps remain visible;
values and repeated instances that depend on execution remain explicitly unresolved.
The planning state `done` may describe an existing or planning-time result and does
not claim that a new run succeeded.

Planning never admits a paid call or starts a workflow run. It may import Python
builders and node modules, execute declared local `at: plan` steps, and write their
results to the cache, just as `plan` does. Python builders use the existing
`file.py:function` contract; arbitrary Python/JavaScript execution scripts are not
loaded to infer workflows. JavaScript-authored workflow files work like other files;
a direct JavaScript builder host remains later work.

To inspect a saved plan without loading any author code:

```sh
grida-fx expand workflows/gallery.yaml --inputs inputs.yaml > graph.json
grida-fx view --plan graph.json
```

The saved plan is validated and held as a static snapshot for this viewer session.
Plan views do not poll, stream events, or replan when the source file changes.

## Open a recorded run

Run viewing shows recorded states, inputs, outputs, failures, costs, and available
artifacts. It reads the selected run and never imports workflow code or calls a provider.

```sh
grida-fx view --run runs/example
```

The Python distribution reaches every viewer mode through the same command:

```sh
python -m grida.fx view --run runs/example
```

All modes start a loopback server on an available port, print its URL, and open
your default browser. It stays in the foreground. Press Ctrl-C to stop the viewer;
a workflow running in another process is unaffected. Closing the browser tab does
not stop the server. Another `view` command starts an independent server.

For a headless machine or your preferred browser, suppress automatic opening:

```sh
grida-fx view --run runs/example --no-open
```

Use `--port 8787` if a fixed local port is useful. An occupied port is an error;
FX never attaches to another service automatically. Only loopback is served.

## Run records and artifacts

The selected directory supplies `plan.json`, `events.jsonl`, and placed output
files. YAML and Python-authored runs use these same recorded contracts; the viewer
does not need to import their workflow modules. A stand-in run is visibly marked.

Select a step to inspect its recorded values and artifacts. Refresh reloads the
record, including changes from an independently running workflow. Images, text/JSON,
audio, and video use generic previews; other file types can be downloaded.

Run canvas cards also show the first available recorded image output. A count
indicates multiple distinct available images; select the card to inspect all outputs.
The canvas uses the verified artifact inventory and never evaluates author code to
produce thumbnails. Plan cards have no artifact previews, including steps marked
`done` by the planner. Refresh updates the cards while preserving the canvas camera.

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
runtime requirements. Contributor builds are described in the repository README.

Contributors can use the [canonical viewer fixtures](../../fixtures/viewer/README.md)
to generate and check real provider-free plans/runs covering branches, matrices,
repeats, conditions, media, cache, failure, and takes. Generated data stays ignored;
the fixture's expectations are the regression baseline.

Project browsing, shared/background servers, real-time updates, automatic hosting
by `run`, custom node views, and portable web
snapshots remain later work. The [TODO](../../TODO.md) records the open ideas.
