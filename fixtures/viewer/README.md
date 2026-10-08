# Canonical viewer development fixtures

This is FX's deliberately broad, provider-free development harness. Its purpose is
to exercise planning, recorded runs, graph projection, layout and inspection as the
viewer evolves. It is separate from practical projects in `examples/`.

The source fixture is small text: workflow files, ordinary Python nodes and explicit
expectations. Pillow draws original geometric images; the Python standard library
synthesizes a short WAV. No external assets, providers, stand-ins or live mode are
needed. Every custom node has a fixture-private explicit version, so different
exports in the shared module retain distinct identities.

## Generate, verify, and inspect

From the repository root, after the normal contributor setup and engine build:

```sh
# Check in temporary storage, then remove generated data.
uv run --project python python tools/check_viewer_fixtures.py

# Keep an inspectable suite. The destination must be new or empty.
uv run --project python python tools/check_viewer_fixtures.py --output .fx/viewer-fixtures/demo

# Saved JSON plans use the compatibility viewer; neither command executes source.
uv run --project python python -m grida.fx view --plan .fx/viewer-fixtures/demo/plans/topology.json --open
uv run --project python python -m grida.fx inspect .fx/viewer-fixtures/demo/runs/cold --standalone --open
```

To inspect a different case, replace `cold` with `cached`, `alternative`, `failure`,
`takes`, `running`, `missing`, `ports`, `ports-legacy`, `dynamic-failure`, `nesting`,
`nesting-failure`, or `nesting-legacy`.
The suite produces ten plans and thirteen run views. Viewing saved data never reruns the workflow.
Each viewer stays in the foreground until Ctrl-C. The running case is a static
recorded prefix; observation does not advance it. For an actual short running workflow:

```sh
cd .fx/viewer-fixtures/demo/project
uv run --project ../../../../python python -m grida.fx run viewer-running --run ../runs/live-local --standalone
```

This command still uses ordinary local code, without `--live`. Its wait is bounded
to five seconds. A new run directory does not force an empty cache; change the
wait input or use another generated project when deliberately testing execution.

## Live observation

`viewer-observation.yaml` is a separate, smaller fixture for attaching during real
execution. Its seven steps form one chain:

`seed → wait → warm → wait_warm → violet → wait_violet → finished`

Three bounded waits separate visibly different local image transforms. Each wait
defaults to five seconds, with a thirty-second maximum (fifteen seconds total by
default). The running highlight moves between waits and completed cards gain image
previews. With a running project service, `run` reports its persistent run URL
before execution starts; add `--open` to open it automatically. Status and artifacts
update through the public observation API. `--standalone` instead hosts a viewer
that stops when `run` exits. Use `inspect RUN --standalone --open` to serve the
retained record independently afterward.

```sh
# Independent CLI and HTTP consumers, checking standalone lifecycle in fresh projects.
uv run --project python python tools/check_observation.py

# Keep the real runs, projections and logs for inspection.
uv run --project python python tools/check_observation.py --output .fx/observation/demo

# Repeat to watch real execution: a fresh project and cache every time.
# Eight seconds per wait, about twenty-four seconds in total.
uv run --offline --no-sync --project python python tools/demo_viewer.py --wait-seconds 8 --open
```

The demo launcher copies only the workflow, project configuration and two node
modules into a fresh temporary directory; it leaves the run there and prints a
command for inspecting it afterward. It delegates to the public `run --standalone` CLI with
dotenv disabled, network off, provider keys removed and a $0 ceiling. `--open` is
opt-in, as it is for the ordinary CLI. Ctrl-C interrupts the run normally.

This fixture has no paid capabilities. Direct `run` invocations with identical
inputs can reuse the waits from cache; the demo launcher always has a fresh cache.
The checker uses `gate: true` and a fixed `.observation-release` marker inside its
copied fixture project, so it can inspect the running state without a timing race.
The marker is development control data, not a workflow event or user-facing pause
API. Releasing the marker unblocks all three waits for the checker. Each node
observes cancellation and fails after its bounded wait if not released.

The independent checker proves snapshot-to-follow continuity, bounded catch-up,
repeatable cursor reads, agreement between CLI and HTTP consumers, a slow observer,
finished-run resume under a new invocation, failure, cancellation, zero spending,
artifact digests, `--no-view`, and hosted-server shutdown. It imports no runtime or
viewer internals. It also verifies the seven-step ordering, wait passthroughs,
changed image digests, and cold execution. Reader unit tests own malformed records
and cursor invalidation.
The existing ten-plan/thirteen-view suite remains unchanged.

## Persistent project service

Prepare a fresh project without starting any processes:

```sh
uv run --offline --no-sync --project python python tools/demo_service.py
```

The helper prints ordinary public commands: `init` has prepared the project, then
use `start --background --open`, `plan viewer-observation --open`, and
`run viewer-observation --name baseline --wait-seconds 8 --max-usd 0 --open`.
The project index and every registered run remain available after execution exits.
A second run with `--wait-seconds 9` gives three new uncached waits at the same
service address. Both runs and the saved plan appear under one workflow; expand
history to see them. `--resume baseline --wait-seconds 8` continues the existing
record without promoting it as a new run; repeating `--name baseline` is an error.
Use `inspect viewer-observation/baseline` for exact selection. These commands
describe source behavior until released. Stop with `stop`; start again to inspect
the same retained URLs.

`cargo test -p grida-fx --test service --test service_run` verifies service lifecycle and execution
independence through the public CLI. Viewer backend and browser tests cover scoped
routing, artifacts, catalog polling and unavailable records. The private catalog
holds local addresses only; portable workflow records stay unchanged. See
[the service contract](../../spec/service.md) for ownership and failure behavior.

## Coverage

| Source/case | What it deliberately exercises |
|---|---|
| `viewer-topology.yaml` | Common image source → independent image/JSON/text/audio branches → a three-image join; a 2×3 Cartesian matrix; three keyed groups with draw/judge/resize; nested group and workflow; list/collection outputs; an ordering-only barrier; long labels |
| Initial topology plan | A free `at: plan` result marked `done`, unresolved runtime conditions, a pending runtime-produced keyed repeat with `max: 3`; 32 instances plus one pending placeholder |
| `cold` | Real completed code-only run: 35 visible instances, including three dynamically discovered instances and a skipped condition; 25 image cards |
| `cached` | Stable output digests and 32 cached executed nodes. The built-in `select` step still records a miss; it is checked separately |
| `alternative` | `enabled: false` reverses the runtime condition and selects the other image |
| `viewer-failure.yaml` / `failure` | An intentional `ctx.fail`, a blocked data consumer, an unrelated successful branch, and a `needs`-only step that still finishes |
| `takes.yaml` / `takes` | A local judge rejects take one, accepts take two, and downstream receives the accepted image; unused third-take instances remain distinguishable |
| `viewer-running.yaml` / `running` | A completed seed, a running delay, and a pending consumer, taken from a genuine execution prefix |
| `missing` | A copy of the real completed run with all placed copies of its sheet image removed; unavailable bytes produce no thumbnail or artifact URL |
| `viewer-ports.yaml` / `ports` | Named image inputs/outputs, two outputs with identical bytes, several wires between the same nodes, list/keyed collections, repeated items and wildcards, computed parameters, node/file facts, literal settings, unused output sockets, and separate ordering controls; 12 nodes and 20 connections |
| `ports-legacy` | The same plan/run with only the new display metadata removed; ordinary dependency edges remain, without invented socket wiring |
| `viewer-dynamic-failure.yaml` / `dynamic-failure` | Runtime-created failure and blocked consumer; both retain named ports and bindings even though the consumer never starts |
| `viewer-nesting.yaml` / `nesting` | Two keyed imports of one wrapper, deeper imports, inline group frames, renamed input/output boundaries, and two public aliases of the same internal image; 11 leaf nodes |
| `viewer-nesting-failure.yaml` / `nesting-failure` | A failed child inside two imported workflow boundaries, blocked inner and outer consumers, an independent success, and failed enclosing cards |
| `nesting-legacy` | The same nested plan/run with display metadata omitted; old dotted and repeated paths stay flat without invented workflow boundaries |

JSON content includes `false`, `0`, `[]` and `null`. The image bundle contains both
a list and a keyed collection, with repeated references to the same three files.
Plan cards never display artifacts, including planning-time `done` results. Plans
and runs carry engine-recorded named port connections. The topology plan has 33
connections and its completed run has 35; runtime expansion adds nodes and
resolves pending references. Data connections remain separate from `needs` and
judge controls.

Those counts describe the complete leaf projection. The viewer initially collapses
imported workflows into cards. The harness checks the root canvas and every imported
workflow separately, including boundary aliases, inline frames, navigation and
failure summaries.

FX follows declared data dependencies: ready sibling steps can run concurrently,
and a join waits for its input values. `needs` adds completion ordering without
consuming a result or requiring it to succeed. This is workflow scheduling; no
cron, webhook or filesystem-watch trigger is implied.

## What the checker proves

`tools/check_viewer_fixtures.py` copies this project into fresh storage, plans every
case and executes the local workflows through the public Python SDK with network
off, dotenv disabled, provider keys removed and a $0 ceiling. It checks real
matrix/keyed expansion, both conditions, media contracts, cache digests, failure
states and accepted-take selection. It uses the public CLI's loopback viewer to
read and verify every available artifact by size and SHA-256.

`tools/check_viewer_fixtures.ts` consumes those actual plans and host responses.
It checks the browser contracts, declared node/edge/state/preview counts, safe
output thumbnails, distinct instance selection, finite nonoverlapping deterministic
layout, exact socket endpoints, parallel wires, and source immutability. It also
checks that matching artifact bytes never replace declared port names and that
the accepted take's instance owns the downstream binding. Scope checks cover exact
parentage, direct leaf ownership, named interface connections, hidden and visible
children, frame containment, breadcrumbs and Back navigation. `expected.json` is the reviewed semantic baseline,
not a generated golden to refresh blindly. Update the workflow and expectations
together when deliberately changing a scenario.

The recorded-prefix fixture keeps exact original event bytes through the delay's
`node_started` event. It is not a fabricated running event or an active process.
The missing-artifact case changes only a copied run. These two derived cases are
explicitly different from a fresh completed invocation.

## Storage and current limits

Generated projects, caches, plans, runs, viewer responses and reports live under the
chosen ignored `.fx/` destination. CI uses temporary storage and checks this suite
after the examples. Keep only the source fixture and expectations in Git. There
is no generated HTML, portable publication format or CDN dependency here.

This first suite covers common topology and viewer states, not every engine feature.
Cancellation/incomplete runs, nested repeats, explicit manual takes, very large
graphs, video/3D and paid-call retries can be added as focused cases. Existing
reader tests own malformed records, traversal, corrupted bytes and torn tails.

Known viewer limits exposed by the suite:

- Old records without port/binding metadata retain ordinary dependency edges.
  Unresolved references and repeat items with ambiguous provenance also retain
  dependency edges until exact evidence exists. The viewer never reconstructs
  port names from filenames, equal content or authored expressions.
- Planning-time false conditions are omitted. A pending repeat has no recorded
  dependency wiring. Runtime-false branches have no skip event; the run reader
  derives their final skipped presentation from the unresolved initial instances.
- Judge verdicts live in facts/reports, separate from successful node execution.
  A rejected valid candidate is not a failed body. Detailed judge/history and
  incomplete-run presentation still need dedicated viewer work.
- Generic inspection currently shows encoded scalar wrappers and flattens file
  collections. Preserve item keys and decode values cleanly in future work.
- Fitting all 35 nodes makes this graph small. Grouping disconnected components,
  focusing selected nodes and handling long titles are concrete layout/inspection
  work the harness makes visible.

Passing this checker is contract and headless layout evidence. Browser rendering,
trackpad behavior and design acceptance require separate visual review.
