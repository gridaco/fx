# TODO

## Standalone workflow and run viewer

**Status:** the local viewer supports workflow plans and recorded runs, with a
read-only node canvas, named typed sockets, exact recorded data bindings, and
separate ordering/judge connections. Imported workflows collapse to named boundary
cards with navigation into their internal canvas; inline groups use labeled frames.
The broader experience
below remains planned; this is primarily UX work, not another engine migration or
a port of an application's viewer. See [Viewing workflows and runs](docs/guide/07-viewing.md).
The [canonical viewer harness](fixtures/viewer/README.md) now supplies provider-free
regression cases. Use it to develop layout for larger graphs, scalar and
keyed-collection inspection, and judge/take history. Extending run responses with
precise incomplete states requires a contract decision;
the client must not infer missing execution metadata.

The contract foundation already exists: expanded `fx-graph-v1` documents, `plan.json`,
`events.jsonl`, placed artifacts, and the state projection exposed by `inspect` and
`project`. Build on [the store contract](spec/store.md) and its schemas. Workflow `view`
metadata is recorded today, but rendering it remains planned; specify any missing viewer
contract before implementing it, without changing execution or identity semantics.

- Provide `grida-fx view` as FX's own local viewer, usable from an arbitrary FX project.
- Make the author-to-result experience clear: inspect the planned graph, browse runs,
  follow step dependencies and states, and inspect artifacts, cache reuse, failures and
  recorded costs. Refresh the display as an existing run progresses.
- Support YAML and Python-authored workflows through the same public FX contracts,
  including custom nodes without application-specific catalog metadata.
- Keep recorded-data inspection read-only and provider-free. Source-target viewing
  uses existing offline planning, including author-code imports and local planning-time
  cache writes, but never starts a run, loads provider credentials or spends money.
- Verify the UX with standalone projects and synthetic offline runs, including a
  stand-in run. Users should be able to understand a run and find its outputs without
  installing an application that consumes FX.

**Prior implementation reference only:**
[Stage Gen's viewer](https://github.com/softmarshmallow/stage-gen/tree/main/web/viewer)
and its run-view projection (`src/stage_gen/runview/`) demonstrate graph and artifact
inspection. Use them to inform the UX; FX's viewer must work independently, with no
dependency on that application's packages, catalog, branding or view-contract namespace.

### Open questions and ideas

The bootstrap uses a foreground process, an independent loopback port, and an
explicit run directory. These defaults do not settle the eventual lifecycle.

- Whether `view` should open/reuse a service and exit, with a separate `view start`,
  background mode, and stop/status commands.
- Project-scoped versus machine-wide services; a machine-local registry of separate
  project services is one possibility.
- Project switching, recent projects, stable URLs, preferences, and version matching
  between a new command and an already-running service.
- How the viewer should expose FX's existing optional `fx.yaml` configuration as
  standalone workflow use grows into a shared project/repository.
- Root discovery, nested workflow homes versus run ownership, explicit standalone
  run opening, and associating runs with stores.
- Project-wide run browsing, automatic refresh, richer graph layout, and custom views.
- Shared browser components for the future website and portable snapshots that
  work with a local file server or any selected storage/CDN provider.
