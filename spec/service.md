# Local FX service

**Status: ratified and implemented in source, 2026-10-08; verified offline.**

The local FX service gives one project a stable loopback address, an index of
recorded runs and saved plans, and the embedded browser client. Workflow execution
remains owned by its invoking process. Both the service and independent observers
read the existing [observation contract](observation.md).

## 1. Command vocabulary

The installed command is `grida-fx` (or `python -m grida.fx`). `fx` in product
discussion is shorthand, not a newly installed executable alias.

| Command | Responsibility |
|---|---|
| `init [DIRECTORY] [--json]` | Establish an optional project by creating a minimal `fx.yaml`. |
| `start [--project DIRECTORY] [--port N] [--background] [--open] [--json]` | Start the local FX service; foreground by default. |
| `status [--project DIRECTORY] [--json]` | Inspect the selected project's service using an identity/compatibility handshake. |
| `stop [--project DIRECTORY] [--json]` | Stop only the selected service; preserve runs, plans, cache and configuration. |
| `logs [--project DIRECTORY] [--lines N]` | Print a bounded tail of the service's local log. |
| `run TARGET [--open] [--standalone]` | Execute; announce a usable run URL when inspection is available. |
| `plan TARGET [--open] [--standalone]` | Plan without run-phase execution; optionally inspect the materialized graph. |
| `inspect RUN [--open] [--standalone]` | Inspect an existing record, optionally in the browser. |

`view` remains a hidden compatibility command for independent foreground viewing
of source targets, saved plans and saved runs. New onboarding uses the commands
above. No service command executes workflow source or loads provider credentials.
Browser opening is explicit through `--open`; browser failures never change the
result of workflow execution. JSON inspection/planning output conflicts with
browser flags that would mix a URL into machine-readable stdout.

## 2. Projects and standalone operation

Existing project discovery is authoritative: the nearest `fx.yaml`, or the
resolved starting directory with documented defaults if there is no project
file. The planning project owns the run catalog, not an imported workflow's home.
`init` is recommended onboarding, not a prerequisite for execution or serving.
Service commands can select their discovery directory with `--project`.

`init` is non-interactive. It creates only a minimal project configuration and
does not overwrite an existing file, install dependencies, load credentials,
start a server or execute a workflow. Repeating it on an existing valid project
configuration is safe. Invalid existing configuration is reported rather than
repaired or overwritten.

With an explicit project, ordinary `run` registers the initialized record and
uses a compatible running service if present. It does not implicitly start a
background process. If the service is absent, execution still proceeds; `--open`
reports how to start the service. `plan --open` and `inspect --open` also require
the selected project's service and do not start it implicitly.

`--standalone` explicitly selects the independent foreground inspection model,
including inside a project. It changes serving lifetime, not workflow resolution,
routes, budgets, cache identity or cache reuse. A projectless `run` retains this
independent model by default. A projectless `plan --open` or `inspect --open`
serves its selected data independently. Ordinary planning/inspection without
browser or standalone flags does not host a server.

For standalone execution, the server uses an available port and ends with the
invocation. Standalone saved-run/plan viewing remains foreground until interrupted.
The compatibility `--no-view` flag skips run viewer integration and conflicts
with `--open`; SDK run methods continue to use it. It does not suppress records.

## 3. Service lifetime and readiness

The foreground process owns the service. `start --background` launches the same
native executable independently, directs its output to a private log, and returns
only after a bounded readiness handshake. A matching already-running background
start is idempotent. Parallel starts are serialized by a lifetime-held OS file
lock; a PID file alone is not proof of ownership or liveness.

Readiness means the server is bound, its local identity and protocol are checked,
and the reported URL is usable. Output describes the actual project, service
instance and address. Failure to bind, incompatible existing service or readiness
timeout is an explicit startup failure. An explicit occupied port is never
silently replaced with a random one, and another project's service is never reused.

The default port is 8787 on `127.0.0.1`. `--port` selects an exact nonzero port;
its successful selection is retained in private local service state across stops.
The OS does not reserve a stopped service's port. A second project may choose a
different port explicitly. Only one instance may own a given project's service.

`stop` uses authenticated local control, not an unchecked PID signal. It is
idempotent for a stopped service and waits at most ten seconds for shutdown.
The server allows two seconds for active HTTP responses to drain, then closes
remaining connections; local runtime cleanup is also bounded. Stopping
the service never cancels workflow processes. The service neither schedules nor
resumes a workflow after a crash. Clients may reconnect and replay recorded events.

The [run-control contract](control.md) defines the separate, runner-owned cancellation
interface. Its private discovery works without this
service; neither service shutdown nor a browser read is a cancellation request.

Background means detached from the invoking command and terminal. Automatic crash
restart, login/boot activation, system-level installation, cross-user access and
machine-wide project aggregation are not part of this first contract. Native OS
supervision can later wrap the same foreground server without changing run events.

## 4. Local catalog and URLs

Private service state lives under the selected project at `.fx/service/`, outside
portable run records. It includes service ownership/readiness information, saved
port preferences, a catalog, saved materialized plans and diagnostic logs. Log reads are bounded to
1 MiB; retained lifecycle logs over that size are reset on the next background start.
Directories and files use private user permissions on Unix. Symlink escapes from
this directory are refused. Absolute source paths and the control credential are
local implementation data; they never enter portable run/plan/event documents,
browser catalog responses, URLs or public logs.

Each catalog entry has an opaque local ID and one of these routes:

```text
/                                    project index
/p/PROJECT_ID/runs/RUN_ID/             recorded run
/p/PROJECT_ID/plans/PLAN_ID/           materialized plan
```

Project identity distinguishes canonical local project roots. Moving a project
changes its local identity; this is not a portable project identity scheme.
A run entry binds a canonical run folder and its initial recorded identity, so
resuming the same record preserves its URL while replacing it is not mistaken for
the earlier run. An invocation ID alone or plan digest alone is not a run ID.
The same plan may have several runs. Saved plans are immutable materializations;
the service never imports workflow code to refresh them.

The catalog supports default project runs and explicitly registered `--run`
locations. Discovery is bounded to the configured run storage, not a scan of a
project's source tree or the machine. Registration is through local CLI filesystem
operations; web requests cannot register arbitrary paths. Missing or changed
records are surfaced without silently pointing an old URL at different data.

The catalog persists independently of server uptime. Registered runs remain
discoverable if they finish while the service is stopped. A restarted service
rebuilds presentation from retained records; it does not store another execution
state machine. Recorded `running` state is not evidence of process liveness.

## 5. HTTP boundary

The existing loopback host, Origin and fetch-site checks apply to all routes.
`GET /api/view` and `GET /api/catalog` at the service root return a
`fx-service-index-v1` document with `project_id`, `title`, and `entries`. Each entry
contains `id`, `kind` (`run` or `plan`), `title`, a relative `url`, and display state.
Neither filesystem locations nor the service's control credential are exposed.

New entries also expose `workflow: {id, source}`, `created_at` (UTC RFC3339), and
an optional run `name`. `source` is the exact portable `workflow.file` recorded in
the graph, including a builder selector when present; it is not a private absolute
path. Unsafe or missing source metadata is omitted, not guessed from a title.
Run creation time/name come from the first `run_started`; legacy run times use the
unchanged plan file's modification time. A saved plan's creation time is its first
catalog registration; registering it again or restarting preserves that time.
Unavailable records retain recorded presentation metadata where available.
These fields affect neither execution/cache identities nor scoped run URLs.
Readers accept older entries lacking this optional metadata.

The index groups by project identity plus recorded workflow ID and source. It does
not group by title or plan digest: changed inputs/definitions at one source stay in
one workflow, while different sources with the same ID remain distinguishable.
Entries without sufficient workflow metadata remain separate. The main workflow
row opens the newest-created run, including failures, or a saved plan when no run
exists. Expandable history includes names, creation times and states, with saved
plans in the same group. Older recorded running/unfinished runs remain visible
without opening history; recorded state still does not prove process liveness.
Resuming never promotes an old run as a new one.

Under a selected entry route, `/api/view`, `/api/snapshot`, `/api/events`, and
`/api/artifacts/DIGEST` retain their existing contracts and validation. Plans have
no run observation or artifact bytes. Artifact URLs are confined to the selected
entry. A wrong project or entry ID returns an error, never the current project's
unrelated run. Browser readers use explicit scoped API bases and reject URLs
outside that scope. The standalone root API remains supported.

The service identity endpoint, `GET /api/service`, and shutdown endpoint,
`POST /api/service/stop`, require the private local Bearer control credential.
Successful identity responses name `fx-service-status-v1`, `protocol_version: 1`,
`state: running`, `project_id`, `instance_id`, `pid`, and the actual `url`.
CLI discovery verifies the expected instance, project and protocol before reuse.
Tokens are not passed on command lines or shown to browsers. This control boundary
does not authorize execution, credential access or arbitrary filesystem reads.

The wire and CLI result schemas are [catalog](schemas/fx-service-index-v1.schema.json),
[status](schemas/fx-service-status-v1.schema.json),
[initialization](schemas/fx-project-init-v1.schema.json), and
[shutdown acknowledgment](schemas/fx-service-stop-v1.schema.json).
`status --json` always returns a status document for an accessible project:
`running` exits 0; `stopped`, `unavailable`, and `incompatible` exit 1. Invalid
configuration or unreadable local state remains a command error (exit 2).
A stopped status has null `instance_id`, `pid`, and `url`. Failed handshakes also
leave those fields null instead of presenting unverified service data.
`init --json` includes the local absolute `project_root`; this is command output,
not a portable workflow record. `stop --json` returns the final status document,
whereas its private HTTP endpoint acknowledges `stopping` before shutdown completes.

## 6. Verification and delivery

The acceptance harness uses only synthetic images and bounded local waits.
[`service.rs`](../crates/grida-fx/tests/service.rs),
[`service_run.rs`](../crates/grida-fx/tests/service_run.rs), and
[backend tests](../crates/grida-fx-viewer/src/service/tests.rs) exercise init preservation; foreground/background readiness; parallel/idempotent
startup; port conflicts; authenticated and project-scoped status/stop; multiple
runs and a plan; scoped artifact bytes; completion after CLI exit; service restart
with stable URLs; execution while the service is absent/stopped; standalone
execution; and SDK capture compatibility. No paid provider, Docker, remote worker
or real browser opener is required in automated tests.

An actual browser check must additionally demonstrate multiple run navigation,
live step changes and completed-run inspection after the producing command exits.
Published packages receive these source behaviors only in a later release.
