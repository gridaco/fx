# Workflow notes

Notes are static, authored documentation with a canvas representation. They belong to a
workflow or a group, not to execution: no attempt, dependency, output, state, cost or
cache entry is created for a note. Values and expressions remain workflow data; notes
do not evaluate `${{ ... }}` or process prompt templates.

## Authoring

The workflow root and group steps may contain an ordered `notes` map. Names follow
workflow member naming rules. Each note has exactly one of `text` (inline string) or
`file` (a local project path), optional `format: markdown | text` (default `markdown`),
and optional `about` naming a direct step in that same container. `about` is a visual
association with a declaration, including its whole deck, never a data/dependency edge.

```yaml
notes:
  overview:
    file: ./notes/overview.md
  reference_help:
    about: draw
    text: |
      ## Reference image
      Use a **clear silhouette**.
  literal:
    format: text
    text: "${{ inputs.prompt }} is an expression example."
```

The file suffix does not select the renderer. Both sources use the same rendering
profile: formatted Markdown or literal text with whitespace preserved. Python and
JavaScript builders produce these same declarations; only the engine resolves files.

Files use the selected workflow's home project and existing project confinement rules,
not the process working directory or the containing YAML directory. Text is strict
UTF-8 with one leading BOM removed, as in [identity.md §5](identity.md#5-text).
The engine bounds note content to 256 KiB. It captures the text once per declaration
for a plan, then projects it into its displayed scope occurrences. Optional materialized
`note_templates` retain per-declaration snapshots (workflow source, container and name)
for groups which expand later. Resume restores this ledger before expansion, including
an empty ledger for old records with no notes; later events cannot reread changed files.
This ledger is local recorded evidence, never part of a public export. A missing, unreadable,
invalid UTF-8 or oversized file leaves a visible unavailable note with a controlled
diagnostic. An unresolved authored `about` likewise keeps the note in its owner scope
with a diagnostic. These presentation failures never turn executable work into failure.
Malformed declarations, including conflicting sources, remain authoring errors.

## Identity and recorded evidence

[Identity §10](identity.md#10-plan-digest) excludes only declared workflow/group `notes`
members from the authored document projection used by the plan digest. Workflows
without notes retain their existing digest. Nothing strips a `notes` key inside `with`,
`tables`, inputs or arbitrary data. A note-only file is not an executable resource.
If the same file is used as a prompt, input or node resource, that use retains its
ordinary identity effect. Editing Python/JavaScript module bytes also retains the
ordinary source/type identity rules; a builder source edit is not universally exempt.

The materialized graph records captured notes. An existing run, including after resume,
shows its original documentation; a fresh plan captures current documentation. Exact
materialized-plan and export evidence hashes distinguish different note snapshots even
when their plan digest matches. A browser never rereads source files to display a run.

Optional `notes` arrays in both graph families, expansion events and the viewer run
projection contain presentation occurrences with `id`, `name`, `scope_id`, `about`,
`format`, and `text` (null when unavailable). `scope_id` is null for root, or an existing
recorded group/workflow scope. `about` is null or the scoped declaration address.
IDs are `note:root:<name>` or `note:<scope_id>:<name>`; they are note references, never
instance IDs. Array order preserves authored order in each owner. Optional `diagnostic`
is controlled plain text. A readable file may carry `source: {path, digest}`, using a
portable project-relative label and the SHA-256 of its original bytes. No absolute path
is recorded. Records without notes retain their existing meaning.

## Presentation and sharing

The engine's layout report gives note cells in a separate optional `notes` map. Existing
execution `cells` and `order` keep their meaning. Note placement is deterministic within
its owner; associations guide placement without changing executable dependency depth or
member order. Stable bounded footprints reserve space before rendering. The authored
layout-file feature is not a prerequisite.

A note card displays a bounded excerpt; selecting it opens full content in the existing
details panel. Notes have no execution sockets, status, timing or cost chrome. Markdown
uses a bundled renderer, with raw HTML and image loading disabled and safe link schemes.
Relative links must not accidentally resolve against a service/run URL. Literal text
preserves characters and whitespace. Rendering never executes workflow expressions.

Portable exports treat note text as author-controlled values: without `--include-values`,
content is withheld explicitly; with it, captured text is included. Local source provenance
is removed. Note structure and placement remain inspectable, including in older recordings
without notes. [Viewer focus](viewer.md) uses an explicit opt-in for note references so
existing protocol-v1 clients never receive an unknown selection kind.

File-backed note source opening reuses [external file activation](viewer.md#external-file-activation)
and [native resolution](files.md): the source must occur in the recorded inventory and
resolve through the host's explicit project binding. Current editable source may differ
from captured note text. No source-serving endpoint or OS action is introduced.
