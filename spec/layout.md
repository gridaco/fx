# Canvas layout

**Status: ratified design, 2026-10-09; Ship A implemented in source, 2026-10-10, unreleased,
with the gaps §12 lists; Ship B not implemented.** In source: the recorded fields of §2, the cell
rule and member order in the engine, the layout route and its report with automatic cells, and
the canvas placing its cards in them with decks. Nothing here is in an installed release, and no
layout file is read. [§12](#12-implementation-order) records evidence and what is left.

This document defines where the bundled viewer places a workflow's steps on its canvas: an
automatic left-to-right grid, decks for repeated instances, and an optional file beside the
workflow that moves steps to authored cells. Layout is presentation. It never changes planning,
identity, caches or resume, and it changes no existing field of any record (§2, §7).

## 1. Terms

| Term | Meaning |
|---|---|
| Container | The root of one canvas, or one declared group in it. A container's cells are computed once and shared by all its frames (§4, rule 1). An imported workflow is one card in its parent; opened, it is a canvas of its own. |
| Frame | One drawn copy of a container: the canvas root, or one iteration or take of a group (a frame card). |
| Declared steps | The `steps` a plan records ([`fx-graph-v1`](schemas/fx-graph-v1.schema.json)), each with its declaration `order` (§2). A container's slots are the declared steps directly inside it. In a document without `steps`, and inside an opened import, the declared steps are the recorded `step` values of its instances and pending entries. |
| Slot | One declared step in one container. In each frame it holds that step's members there, or a placeholder (§5.1). |
| Cell | A slot's column and row in its container, counted from 0. |
| Member | One instance (node card), one group iteration or take (frame card), or one imported workflow occurrence (workflow card) in one slot of one frame. Absent instances are never members. |
| Deck | The members of one slot in one frame, n ≥ 2, drawn as one stack (§5); spread instead when the file says `repeats: grid` (§6.3). |
| Address | A declaration path: step names joined by `.`, with no repeat key and no take. `badge.draw` addresses every `draw` inside every `badge`. |
| Order key | The canonical order of slots and members (§4, rule 4). |

## 2. Two ships

| | Ship A: automatic grid and decks | Ship B: the layout file |
|---|---|---|
| Delivers | the cell rule and the member order (§4, §8), decks and their interaction (§5), stable live updates, socket edges (§9), the scoped layout route and its report with automatic cells (§6.11) | `<workflow id>.layout.json` (`fx-layout-v1`, §6), diagnostics, the report's file members, `inspect` output, grid spreads and matrix tables (§6.3) |
| Code | `crates/grida-fx-core` and `crates/grida-fx-runtime` record the added fields; `crates/grida-fx-viewer` reads them, computes cells and the member order and serves the report; `js/fx-web` places its own projection's cards in the served cells and draws; `web/viewer` | `crates/grida-fx-viewer`: the file reader, discovery and diagnostics; `crates/grida-fx`: `inspect`; `crates/grida-fx-core` and `crates/grida-fx-runtime`: recorded matrix axis values; `js/fx-web`: grid spreads and matrix tables |
| Contracts | optional members only. [`fx-graph-v1`](schemas/fx-graph-v1.schema.json): `order` on `steps` entries; `step` and `waiting_on` on `pending` entries; a materialized plan (`plan --open`, `plan --standalone`, `view` of a workflow) carries `steps` and `takes_file`, as `plan.json` does. [`fx-run-events-v1`](schemas/fx-run-events-v1.schema.json): `scopes_updated` adds `instances` (each expanded instance's `id`, `step`, `take` and `key`, in expansion order; absent instances, never members, are left out) and `pending` (the expansion's pending repeats, as in fx-graph-v1). [`fx-viewer-run-v1`](schemas/fx-viewer-run-v1.schema.json): nodes add `step`, `take`, `key` and `interrupted`; the document adds `pending`, the repeats not yet expanded. New: the scoped layout route and the `fx-layout-report-v1` kind, whose schema Ship A writes | [`fx-layout-v1`](schemas/fx-layout-v1.schema.json); the report's file members; `axes` (a matrix combination's axis values, in declared axis order) on fx-graph-v1 instances, `scopes_updated` `instances` and fx-viewer-run-v1 nodes |
| Removes | `@dagrejs/dagre` from `js/fx-web`; no ELK dependency is added | nothing |
| Never changes | any existing field or its meaning, the workflow format, the plan digest, step, type and call identity, the store, caches, resume | the same; planning, runs, resume and the store never open the layout file |

The ratified split kept Ship A to the viewer and additive contract fields. Computing cells in the
engine (§8) widens that: a plan's `/api/view` stays the verbatim fx-graph-v1 document, so Ship A
also brings the scoped layout route and the report kind, and it records the step, key, order and
pending facts that the cell rule, the badges and the run view read and that no record holds
today.

[Agent readiness](../AGENT_READINESS.md) scenario AR-17 becomes current only with Ship B.

## 3. Inputs

The cell rule reads these, and only these:

- each container's declared steps (§1);
- the placement edges (§4, rule 3);
- the order key (§4, rule 4);
- in Ship B, the applied layout file.

Members, state, cost, errors, artifacts, selection, hover and expansion are never inputs to
cells; members decide decks and footprints (§5). Sizes are not inputs to cells; they only size
tracks (§4, rule 11).

## 4. Automatic cells

Rules 1–8 compute an automatic cell for every slot, as if no file existed. Rule 9 then places
authored cells, rule 10 removes empty tracks and rule 11 sizes them.

1. **Containers.** The root, then every declared group, inside out. All frames of a group (its
   iterations and takes) share one cell set, computed over their union: the group's declared
   steps and every frame's placement edges. Each frame draws every slot of its container, as a
   member, a deck or a placeholder (§5.1). A group's inside is laid out first; the group is then
   one slot of its parent container.
2. **Slots.** One per declared step. A slot's key is the recorded declaration path (`step` in
   [`fx-graph-v1`](schemas/fx-graph-v1.schema.json)), never a path parsed from an instance id
   ([identity.md](identity.md) §11).
3. **Placement edges.** One set for the plan view and the run view: the plan's bindings, `needs`,
   `judges`, `reads` and `waiting_on`, plus edges a run recorded that the plan lacked. A recorded
   edge may add a predecessor, never remove one. An edge counts in the innermost container that
   holds both ends, between the slots that contain them. Edges inside one slot drop.
4. **Order key.** Slots go by their declared `order`. Members of a slot go by their position in
   the newest recorded expansion order: the `instances` of the latest `scopes_updated`, else the
   plan's `instances`. A frame or a collapsed workflow takes the smallest key among its
   descendants. Arrival and scheduling order are never used. A record without Ship A's fields
   (an older run or plan) falls back: slots go by
   the first index of their instances in the plan's `instances`, then slots known only from
   `pending`, in that array's order; members missing from every recorded expansion order go by
   `key` in code-point order, then by `take` compared number by number.
5. **Clusters.** The connected components of the container's slots, edge direction ignored. The
   largest comes first, by slot count; ties go by order key. Rules 6–8 run per cluster, and each
   cluster's columns then follow the previous cluster's last column. All clusters share the
   container's rows.
6. **Depth.** As soon as possible: 0 with no predecessor, else 1 + the largest predecessor depth.
   In a cycle, an edge to a slot earlier in the order key is ignored for depth and still drawn.
7. **Row.** Depths are placed in increasing order. In each depth, slots sort by preferred row,
   then order key. The preferred row is the smallest row among a slot's predecessors, 0 with
   none. A slot takes its preferred row if free, else the next free row below.
8. **Wrap at 3.** A depth with more than 3 slots splits into ceil(n/3) sub-columns of up to 3
   rows, filled top to bottom in rule-7 order; a depth of 3 or fewer is one column and keeps rule
   7's rows. Each depth is wrapped before the next depth's rows are chosen, so successors read
   final rows. Depth 0 starts in the cluster's first column, and each depth starts in the column
   after the previous depth's last sub-column. The constant is the engine's; no file key changes
   it.
9. **Authored cells** (Ship B). Each slot the applied file places moves to its authored cell,
   and no other slot follows it. Authored values and automatic cells share one coordinate space:
   the container's columns and rows before rule 10. Authored slots are placed first, in order
   key; two that name one cell keep the first there and move the other (`shared_cell`). Then
   each automatic slot whose cell is taken, in order key, moves to the next free row below in its
   column (`displaced`). Wrap counts automatic slots only and does not run again, so a column may
   now hold more than 3 slots. An edge with an authored end whose target column is at or before
   its source's is drawn as a backward edge (§9) and reported (`backward_edge`). An empty file
   draws exactly what no file draws.
10. **Empty tracks collapse.** A column or row that holds no slot is removed; order is kept.
11. **Pixels.** Tracks span the container: a column is as wide as its widest slot footprint, a row
    as tall as its tallest, over every frame of the container, so a group's frames are the same
    size. Gaps are 100 between columns and 42 between rows, in canvas units. Content starts top
    left.

Rules 1–10 read no size, so a preview arriving never changes a cell. Footprints come from
declarations: ports, settings and kind. A step whose declared output ports include an image
reserves its preview area in both views, before any artifact exists.

Example: `seed → propose (2 takes) → warm, cool, mono, dusk, night → sheet`, and an island
`notes → index`.

```
           col 0   col 1        col 2    col 3      col 4    ┆   col 5    col 6
 row 0     seed ─▶ propose ──┬▶ warm     dusk  ──┬▶ sheet    ┆   notes ─▶ index
                   [2 takes] │                   │           ┆
 row 1                       ├▶ cool     night ──┤           ┆
 row 2                       └▶ mono  ───────────┘           ┆
           └─ cluster 1: cols 2-3 are one wrapped depth ───┘ ┆   └─ cluster 2 ─┘
```

`propose` feeds all five variants; its edges to `dusk` and `night` pass the column-2 cards. All
five feed `sheet`, which takes the column after the last sub-column and its predecessors'
smallest row. The island is smaller, so it comes second, in the columns after the chain's and on
the same rows.

With a file that places the island under the chain (Ship B):

```json
{ "kind": "fx-layout-v1",
  "steps": { "notes": { "column": 0, "row": 3 }, "index": { "column": 1, "row": 3 } } }
```

```
           col 0    col 1        col 2    col 3      col 4
 row 0     seed  ─▶ propose ──┬▶ warm     dusk  ──┬▶ sheet
 row 1                        ├▶ cool     night ──┤
 row 2                        └▶ mono  ───────────┘
 row 3     notes ─▶ index
```

Both island slots leave their cluster's columns; columns 5 and 6 are left empty and collapse.
With only `notes` placed, `index` would keep its automatic cell, column 6, which becomes column 5
once the emptied column collapses.

## 5. Decks

### 5.1 What a deck is

- The members of **one slot in one frame**, n ≥ 2. A deck never mixes two steps and never crosses
  frames.
- Node cards: for_each items, matrix combinations, takes and rerolls. Frame cards: iterations and
  takes of a repeated or regenerating group. Workflow cards: repeated imports.
- n = 1: one card labelled with its recorded `key` or take, no stack.
- A slot with no member in a frame draws one placeholder card at the slot size: `0 items`;
  `absent`, when its instance is recorded absent; or, before expansion, `up to 8 items` (the
  pending `max`), joined to what its pending entry records waiting on.
- A slot whose step the file sets to `repeats: grid` (Ship B) spreads its members instead (§6.3).

```
  5 items · 1 failed                    click ▶   ┌────────┐ ┌────────┐ ┌────────┐
  ┌────────────┐                                  │  ada   │ │   bo   │ │   cy   │
  │ ┌────────────┐                                └────────┘ └────────┘ └────────┘
  │ │ ┌────────────┐                              ┌────────┐ ┌────────┐
  └─│ │ entity     │                              │   di   │ │   ed   │   Esc / × / outside ◀
    └─│ draw review│ ◀ front card (§5.4)          └────────┘ └────────┘
      └────────────┘
```

### 5.2 Nesting

Items are the outer level and takes the inner one. A regenerating step inside a `for_each` is a
deck of item cards, each holding a deck of its takes. In the
[rigged-character example](../examples/rigged-character/) with `partition: head_body`, `body` has
up to 3 takes and holds `part`, one item per role with up to 3 takes each. `body` is a deck of
frame cards; each frame holds `part` as a deck of 2 item cards, each a deck of takes, badged
`2 items · 3 takes each`. At rest, when §5.4 rules 1–3 do not apply, a take deck shows the take
consumers bind (§5.4, rule 4).

### 5.3 Footprint and badge

- A deck draws its front card and at most **3** back silhouettes, without content, each offset
  28 canvas units right and 28 down from the card in front of it. Every member draws at the slot
  size: the largest declared member footprint.
- Footprint: width w + 28 × min(n − 1, 3) and height h + 28 × min(n − 1, 3), plus a reserved
  badge band. It stops growing at n = 4; the badge carries n. Tracks are sized from footprints
  (§4, rule 11), so a deck never reaches a neighbouring slot.
- Badge text: `5 items`, `3 takes`, `2 items · 3 takes each` (`up to 3 takes` when item decks
  differ), then `· 1 failed` in the failed colour and `· 1 not run` for takes that never started.
  Counts come from recorded state and recorded take arrays, never from id strings.
- The badge sits inside the footprint, scales with the canvas and uses theme tokens. Too small to
  read, it shows `×5`; smaller still, it hides and a failed count stays as a colour dot.
- Accessibility: the deck is one group named `<step title>, 5 items, 1 failed, collapsed`; the
  badge is `aria-hidden`. Each card adds its key or take and position: `item 'ada', 1 of 5`,
  `take 2 of 3, skipped`.

### 5.4 Resting front card

The front card at rest is the first that applies; "first" and "last" are by order key:

1. the selected member;
2. the last failed member; in the plan view a `blocked` member counts as failed;
3. the first running member (run view);
4. in a take deck, the take consumers bind: the take that instances outside the deck record
   reading (`reads`, and in the run view `bindings`). That is the picked take (`pick:` or the
   takes file), or for a regenerating step its last take: the accepted one, or the one `then:`
   kept. When nothing outside the deck reads one of its takes, this rule does not apply;
5. the last succeeded member (`done` in the plan view);
6. the first member.

A member that never started never rests over one that did. In the plan view, `planned` and
`maybe` members have not started; in the run view, `pending` and `skipped` ones (the read model
also marks members that never started `skipped` once the run ends).

### 5.5 Linked decks

Two decks in one frame whose members correspond one to one, by item key or by take, are linked.
§5.4 runs once over the pairs, and a pair matches a rule when either of its members does; both
decks show the winning pair in front. A review deck shows the review of the draw in front.
Hover raises a card's partner with it.

### 5.6 Edges on a deck

Member edges merge by source slot, source port, target slot and target port, in both directions:
edges leaving a deck, edges fanning into one (`seed` to every `draw` item), and edges between two
decks (each `draw` item to its `review`). On a closed deck, each merged end attaches to the front
card's socket. A merged edge is labelled with its count of member edges, or with the member's key
when only one member connects and it is not in front. Expanded, each card shows its own edges,
including those that leave the deck.

### 5.7 Interaction

| Input | Closed deck | Expanded deck |
|---|---|---|
| Hover, only where `(hover: hover)` | raise the card under the pointer, and its linked partner; leaving restores the resting card | nothing; everything under the veil is inert |
| Still click on a card, a back card's visible rim or the badge | expand | select that member |
| Click a labelled control on the front card (`Open workflow`) | does what it says | does what it says |
| Double-click | counts as one expand click | on a workflow card: open it |
| Click the veil | — | collapse one level; the selection is unchanged; a second deck needs a second click |
| Esc | with focus in the canvas: go back a scope (unchanged) | collapse one level, wherever focus is except in a text field; consumed only then |
| × | — | collapse one level |
| Tab | one stop per deck (the front card); back cards have `tabindex=-1` | each card, in grid order |
| Enter or Space | expand | select |
| Drag past 4 px | pan, from anywhere | pan, from anywhere, the veil included |
| Touch, first tap | expand | select |
| Select from the step list or a reference link | raise the target to the front; no expand, no camera move | collapse first if the target is outside |
| Scope change | — | collapse at once; keep the pre-expand camera |
| A deck inside an expanded card | — | a click expands it one level deeper. Only the innermost level is interactive; the levels around it are its veil, so a click on them collapses one level and selects nothing |

On collapse, focus returns to the deck's front card. Raising a card never moves the focused
element in the document. At most one deck per view is expanded, plus its nested levels. There are
no dedicated deck buttons, no subview and no URL that selects a member (§11).

### 5.8 Motion and live updates

- A deck's state (closed or expanded) changes at once and in full. Transitions only interpolate
  transform and opacity: 220 ms ease, and 0 ms under `prefers-reduced-motion: reduce`. Stacking
  order is never animated. Input during a transition jumps to its end state.
- Expanded, a deck lays its cards out in a grid whose columns come from the viewport's aspect and
  the card size. The grid's top left is the deck footprint's top left, and it draws above the
  veil. Expanding changes no cell and no track: neighbouring slots stay where they are, under the
  veil. Expanding shows the veil, slides the cards into the grid and glides the camera.
- Expanded, hovered and resting state is viewer state keyed by scope, frame and step. It
  survives updates.
- An update whose layout inputs (§3) are unchanged patches state only: classes, labels and counts.
  Elements, hover and highlight persist. New members join an open deck in order and the camera
  stays. A deck that vanishes or drops below 2 members collapses and restores the camera.
- Tracks never shrink within one observation.
- A run's snapshot carries the report of its own prefix (§8), so cells and structure never lag
  each other.

### 5.9 Camera

- Expanding fits the grid, but not below a legibility floor (card titles about 11 px on screen).
  Below it, the camera aligns to the first row and the rest is reached by panning. A grid already
  readable only pans.
- Collapsing restores the camera relative to the collapsed deck.
- The automatic fit refits on every layout change until the viewer pans, zooms, selects or
  expands. After that, a layout change keeps an anchor on screen: the selected box, else the box
  nearest the centre. Status-only updates never refit.

## 6. The layout file (Ship B)

### 6.1 Vocabulary

```json
{
  "kind": "fx-layout-v1",
  "steps": {
    "sheet":      { "column": 4, "row": 1 },
    "variant":    { "repeats": "grid" },
    "badge.draw": { "column": 0, "row": 1 }
  }
}
```

[`fx-layout-v1`](schemas/fx-layout-v1.schema.json) is the whole vocabulary: `kind`, and `steps`
mapping an address to an entry with `column`, `row` and `repeats`. Here `variant` is a matrix step.
There is no `defaults`, `clusters`, `wrap` or pin key.

### 6.2 Keys

- A key is an address, matching `^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*$`: step names
  ([`fx-workflow-v1`](schemas/fx-workflow-v1.schema.json) `name`) joined by `.`.
- A key with a repeat key (`[…]`) or a take (`#`) fails the pattern, and the message names the
  step it would cover.
- One key covers every instance of its step, in both views, at any count.

### 6.3 Entries

- **`column`, `row`:** integers from 0 to 9999, given together. They place the slot in its
  container's coordinates (§4, rule 9): `badge.draw` places `draw` inside every `badge` frame.
  Empty tracks collapse (§4, rule 10), so 10, 20, 30 work, and inserting a row needs no
  renumbering.
- **`repeats`:** `deck`, the engine default, or `grid`. `grid` spreads the slot's outer level
  instead of stacking it: its items, matrix combinations or group iterations, else its takes. An
  inner take level stays a deck inside each spread card. A spread matrix is a table: one column
  per value of its last axis, one row per combination of the others, from the recorded `axes`
  (§2). Other spreads, and a record without `axes`, use ceil(√n) columns, n the outer member
  count. On a step that never repeats it has no effect.

### 6.4 Precedence

A step's entry, then the engine default. No `defaults`, no selectors, no inheritance.

### 6.5 Reader

- The file is UTF-8 [I-JSON](identity.md#1-values), at most 1 MiB: a duplicate key is refused.
- It must validate against [`fx-layout-v1`](schemas/fx-layout-v1.schema.json), which allows no
  other member.
- An invalid file is not applied at all: every view and `inspect` draw the automatic layout and
  report `refused` until the file is valid again. The engine keeps no earlier revision. A writer
  renames a complete temporary file into place, so no reader sees it half written.
- Optional members may be added within v1. A changed meaning is v2.

### 6.6 Discovery

- The file is `<workflow id>.layout.json`, in the folder of the workflow's takes file
  ([store.md](store.md) §8, [protocol.md](protocol.md) §5.2). Planned in its own project,
  `workflows/concept-gallery.yaml` keeps its takes in `workflows/concept-gallery.takes.yaml`, so
  its layout file is `workflows/concept-gallery.layout.json`. A workflow whose home is another
  project keeps its takes, and so its layout file, in the planning project's root.
- The location comes from the recorded `takes_file` alone, in both views: `plan.json` records it
  for a run, and from Ship A a materialized plan records it too (§2). It is project-relative and
  joined to the serving project's root, as `pick` finds a takes file. A document without
  `takes_file` (a plan materialized before Ship A, or one printed by `expand`) has no location
  (`layout_location_unknown`).
- The location is derived, never requested: a browser never names a path
  ([service.md](service.md) §5). A `takes_file` that is not a plain relative path inside the
  project, or a location reached through a symbolic link that leaves it, is refused
  (`layout_location_refused`).
- No file means automatic layout.

### 6.7 Builders

A builder's file has the same name, in the folder of its takes anchor ([protocol.md](protocol.md)
§5.2), which the recorded `takes_file` names. The reader checks it against the recorded declared
steps; the builder is never imported.

### 6.8 Imports

A file arranges only its own document. The importer's file places the collapsed import by its
step's address. Inside an opened import the layout is automatic in v1.

### 6.9 Historical runs

A run uses the current file, checked against the run's recorded declared steps. A file written for
a newer version of the workflow never blocks an older run's view; mismatches are diagnostics.

### 6.10 Diagnostics

Diagnostics are structured and never block the view.

| `code` | When | Effect |
|---|---|---|
| `unknown_step` | the key names no declared step of this document (§1) | ignored |
| `not_drawn` | the step is declared, but its container has no frame in this view (its group is absent or has 0 items) | kept, no effect |
| `outside_document` | the key reaches inside an imported workflow | ignored |
| `shared_cell` | two keys name one cell | the slot later in the order key moves to the next free row below; both named |
| `backward_edge` | an edge with an authored end has its target column at or before its source's | honoured, the edge named |
| `displaced` | an automatic slot moved because an authored cell took its place | reported |
| `layout_invalid` | an I-JSON, size or schema failure | not applied (`state: refused`); JSON pointers name each failure |
| `layout_location_unknown` | the document records no `takes_file` | automatic layout (`state: none`) |
| `layout_location_refused` | the recorded `takes_file` is not a plain relative path inside the project, or the location leaves it through a symbolic link | not read; automatic layout (`state: refused`) |

In a document without `steps` (§1), a key for a step with no instance there reports
`unknown_step`.

### 6.11 The layout report

One report answers for the selected run or saved plan: from the scoped route `<entry>/api/layout`
and the standalone root's `/api/layout`, as the `layout` member of a run's `/api/snapshot` (§8),
and, from Ship B, as the `layout` member of
`inspect RUN --json`. Ship A writes its schema and serves it with `file` and `revision` null,
`state: "none"` and no diagnostics; Ship B fills those members.

```json
{
  "kind": "fx-layout-report-v1",
  "file": "workflows/gallery.layout.json",
  "revision": "9f2c…",
  "state": "applied",
  "cursor": "…",
  "diagnostics": [
    { "code": "unknown_step", "address": "preview", "message": "no step 'preview' is declared in this workflow" }
  ],
  "cells": {
    "seed":    { "column": 0, "row": 0, "source": "automatic" },
    "propose": { "column": 1, "row": 0, "source": "automatic" },
    "sheet":   { "column": 4, "row": 1, "source": "authored" }
  },
  "order": ["seed#1", "propose#1", "propose#2"],
  "decks": [{ "address": "propose", "frame": null, "items": null, "takes": 2, "repeats": "deck" }]
}
```

(The §4 example with `sheet` moved down one row; other cells and members left out.)

- `file`: the derived project-relative POSIX path, whether or not a file is there; null when no
  location is known (`layout_location_unknown`) or the recorded `takes_file` is not a plain
  relative path.
- `revision`: the file digest ([identity.md](identity.md) §2) of the bytes read, or null when
  nothing was read. A refused file has a revision too, so an agent's edit is live only when
  `state` is `applied` and `revision` equals the digest of the bytes it wrote.
- `state`: `applied`, `none` (no file at the location, or no location) or `refused`
  (`layout_invalid` or `layout_location_refused`).
- `cursor`: for a run, the [observation](observation.md) cursor of the record prefix the report
  reflects; null for a plan.
- `cells`: every slot's address mapped to its cell and `source`, `automatic` or `authored`,
  container by container from the root. A container's frames share its cells (§4, rule 1), so an
  address is an unambiguous key. Addresses inside imported workflows are listed too; their cells
  are always automatic (§6.8).
- `order`: every member, as instance and scope ids, in order key (§4, rule 4). Only the relative
  order of one slot's members means anything; the browser orders each deck by it.
- `decks` (Ship B, for `inspect`): one entry per slot and frame with two or more members at either level, stacked or
  spread: `address`; `frame`, the frame's scope id, null at a canvas root; `items`, the outer
  member count, null without an item level; `takes`, the largest take count of one item (or of
  the slot, without an item level), null without a take level; and `repeats`.

FX never writes the file. Writing every reported cell back as authored draws the same picture
(§4, rule 9).

## 7. Identity

- Planning, `expand`, `run`, resume and the store never open the layout file. It is not part of
  the [plan digest](identity.md#10-plan-digest): adding, editing, corrupting or removing it
  changes no digest, no step, type and call identity, and no cache key.
- No run record names the file, and a run folder holds no copy of it.
- The workflow document is untouched: its `view` field and the builder `Workflow` carry no
  layout.
- Ship A's and Ship B's recorded fields (§2) are optional additions: none enters the plan digest
  or an identity, and a record without them still draws, by the fallbacks in §1 and §4.
- Proof (Ship B): a conformance case plans and runs a workflow, then adds, edits and corrupts its
  layout file. `expand` prints byte-identical output, the plan digest stays the same, and a
  same-plan resume of the run folder is accepted with every step a cache hit.

## 8. Engine split

| Part | Kind | Owner |
|---|---|---|
| connected components, topological order, cycle detection | generic | a Rust graph library, such as `petgraph`; its version and licence are checked when it is added |
| slots, order key, clusters, depths, rows, wrap, authored cells, empty-track collapse, diagnostics | product | `crates/grida-fx-viewer` |
| per-scope projection: canvases, frames, members, collapsed imported workflows, rerouted and merged edges with their ports | product | stays in `js/fx-web` (`scopes.ts`, `graph.ts`); its cards are keyed by the recorded `step`, the report's cell key |
| parsing and validating documents; node card content by id (title, ports, settings, preview, state); navigation and selection state | presentation | stays in `js/fx-web` |
| track pixels, deck footprints, edge curves, motion, camera | presentation | `js/fx-web`, from the served report |

- **Native.** `inspect` must report cells, and the installed command has no JavaScript runtime.
  The browser places its cards in the cells the engine serves; it never computes a cell.
- **Pure.** The same records and file give the same report on every viewer and every poll. No
  history, no asynchronous work. Budget: cells and order for 500 nodes within 16 ms.
- **Serving.** Every host that serves a view also serves its report: the standalone root at
  `/api/layout`, and the project service under the selected entry's scope. Responses are
  `Cache-Control: no-store` and carry no `ETag`: the body changes as a run records more while the
  file stays the same. The route reads only recorded documents and, in Ship B, the derived
  layout file.
- **Refresh.** The browser places cards by the newest report's cells and orders decks by its
  `order`; it draws structure and node content from its own snapshot and events. A run's
  `/api/snapshot` carries, beside its `view`, the report of exactly the same captured prefix as
  its `layout` member, so a live update reads and projects the record once and the report's
  cursor is the snapshot's. A saved plan's report is read from `/api/layout`. A slot the report
  does not place, as under a host that serves none, waits in a fallback column. From Ship B,
  every view, saved plans included, also fetches the report every 2 s while the page is visible,
  since an edited file emits no event.
- **Owned code.** No layout library places shared-track cells with decks and wrap. ELK layered
  keeps columns but treats rows as a preference, and holds authored rows only when handed final
  coordinates: it can reproduce cells, not compute them. dagre, which the canvas used before Ship A, with
  its default network-simplex ranking, minimises total edge length, so a step with one consumer
  can be placed late, next to that consumer, and its coordinates depend on node sizes. The cell
  rule is therefore FX's own code; only its generic graph algorithms come from a library.

## 9. Edges

- A forward edge is a cubic curve between sockets with horizontal tangents at both ends.
- A backward or same-column edge is a rounded polyline through waypoints in the gaps between
  rows, computed from the cells and tracks.
- Cells are authoritative. A later router may adjust pixels only.
- The browser carries no layout library.

## 10. Edge cases

| Case | Rule | Ship |
|---|---|---|
| A deck inside a deck's card (`body` × 3 takes holding `part` × 2 items × 3 takes) | each card is its own stacking context; inner decks draw inside; expand one level per click | A |
| Mixed member states; a cancelled run | count by recorded state; rest by §5.4; a step the run ended before it finished carries `interrupted` on its run-view node (§2), never parsed from error text | A |
| Members arrive one by one | the full count as soon as `scopes_updated` lists them with their step (§2); listed members that have not started draw as not started; never reordered by arrival. In a record without that field, members join as they start | A |
| A pending repeat, plan or run | an `up to N items` placeholder in its slot in both views, from the recorded pending `max`, joined by recorded `waiting_on` | A |
| 0 or 1 items; an absent step | a `0 items` or `absent` placeholder, or one keyed card; the slot and its cell stay the same in both views | A |
| 200 to 10,000 items | at most 3 back silhouettes; an expanded deck renders only visible cards; the step list groups members | A |
| A preview arrives; a fact row is added | the slot is sized from declarations with its preview reserved; tracks never shrink within one observation | A |
| A long chain | only depths of more than 3 slots wrap; a chain never snakes; the camera handles width | A |
| A frame whose members are all absent or not started | each slot draws its placeholder and the frame keeps its container's size; the deck count is the scope count | A |
| Repeated imports | one deck; the first click expands; `Open workflow` on a card does what its label says | A |
| Two runs schedule items differently | the order key reads recorded expansion order, never arrival (§4, rule 4) | A |
| A reroll deck that starts at take 4 | labels come from recorded take arrays | A |
| A status-only update; a live update or scope change while expanded; input during a transition; zoom at 11 %; an oversized expanded deck; Esc with focus outside the canvas | §5.7 to §5.9 | A |
| An uneven matrix spread (2 × 4) | a table by its last axis, from recorded `axes` | B |
| One authored cell | only that slot and the slots it displaces move; `displaced`, and `backward_edge` for an edge it turns backward | B |
| An authored cell in another cluster's columns | allowed: cells span the container; the slot leaves its cluster, the rest of the cluster stays, and emptied tracks collapse (§4 example) | B |
| A renamed or removed step, an old run | `unknown_step`, or `not_drawn` when its container has no frame; never blocks | B |
| A half-written or invalid file | not applied: automatic layout in every view and in `inspect`; `layout_invalid` | B |
| A layout edit, then resume | not in the digest (§7) | B |
| An override for one instance | the key pattern refuses it | deferred |

## 11. Deferred

- Keys for one instance (a repeat key or a take).
- `defaults`, `wrap` and `clusters` keys; pins.
- Labels and descriptions; visual groups.
- An imported workflow's own layout file.
- Per-run layout files; a CLI option or URL parameter that selects a file.
- A URL that selects or raises one member.
- Portable snapshots that carry their layout.
- A command that writes or renames the file; visual editing; a minimap.
- Typed SDK access to the layout report. From Ship B, Python and JavaScript callers receive it
  untyped inside the existing `inspection` (the `inspect --json` summary).
- ELK or any other router over the cells.

## 12. Implementation order

Ship A, then Ship B. Each step lands with its evidence, recorded here.

**Ship A: automatic grid and decks.**

1. Record the added fields (§2) in `crates/grida-fx-core` and `crates/grida-fx-runtime`, with
   their fx-graph-v1, fx-run-events-v1 and fx-viewer-run-v1 schema edits. Re-record the
   conformance cases whose expected documents hold `pending` entries, `plan.json` `steps` or
   `scopes_updated` events, and check that each difference only adds members.
   *Landed.* Evidence: unit tests of pending `step` and `waiting_on`, steps' pre-order `order`,
   `scopes_updated` `instances` (absent ones left out, members listed before they start) and
   `pending`, and the materialized plan's `steps` and `takes_file` (`view` of a workflow or a
   builder, `plan --open`); eleven re-recorded conformance cases (`phase`, `refusals`,
   `stand-in-image`, `identity-once`, `run-failures`, `run-retry-engine`, `run-timeout`,
   `stand-in-agent`, `stand-in-errors`, `stand-in-flags`, `stand-in-job`), whose every
   difference adds a member and no event.
2. In `crates/grida-fx-viewer`: the read model's added node fields, `interrupted` and `pending`;
   the cell rule and the member order; the scoped layout route with the report's schema and
   automatic cells.
   *Landed.* Evidence: `layout` unit tests (the §4 example, columns of three, a 30-step chain
   in one row, a cycle, group frames sharing cells with edges lifted to the common container,
   a pending repeat, member order without absent members, the fallback without declared
   steps, an empty step address, a repeat that keeps its plan cell in the run view after it
   expands to no item, and 500 instances twice byte-identical within budget); the route test validates a plan's
   and a run's report against `fx-layout-report-v1`, `no-store` and no `ETag`; `check_viewer`
   reads `api/layout` from the installed engine in plan and run modes, and a run's report carries
   its cursor; a run's `/api/snapshot` carries the same report as `/api/layout`.
3. Place the projection in the served cells in `js/fx-web`: tracks, deck footprints, placeholders, badges,
   edges, interaction, motion, camera and refresh; remove `@dagrejs/dagre`.
   *Landed in part.* Cards go in the served cells with tracks over every frame of a container,
   and a card with a declared image output reserves its preview area in both views; absent
   instances never join a deck; decks with at most three back cards, the badge (with its short
   and dot forms at low zoom), the §5.4 resting card, hover raise and restore, click to expand
   in place over a veil with the camera gliding down to 11 px titles, Esc, × or a click outside
   to collapse with focus back on the front card, a labelled control on a card doing what it
   says, a drag past 4 px panning from anywhere, 220 ms transitions and none under reduced
   motion; an open deck stays open across live updates, and one that vanishes gives the camera
   back; forward edges are socket curves and backward edges rounded polylines; dagre removed
   (bundle 352 → 317 kB). Evidence: placement, preview reservation, deck badge and
   resting-card unit tests; the fixture check allows only deck cards to overlap. Not yet:
   placeholders for absent, empty and pending slots in the run view; merged edges on a closed
   deck (§5.6); linked decks (§5.5); take decks nested inside item cards (a node deck is one
   stack in item-then-take order, with the two-level badge); expanding a deck inside an
   expanded card (§5.7, nested levels); a double-click on a closed deck counting as one expand
   (§5.7); the deck's and its cards' accessible names (§5.3); keyboard Tab order inside an
   expanded deck; patching an update whose layout inputs are unchanged instead of redrawing,
   which today drops hover and highlight, and tracks that never shrink within one observation
   (§5.8); the automatic refit, the camera anchor after a layout change and a collapse
   relative to the collapsed deck (§5.9). No one has checked the canvas in a browser yet.
4. Update the viewer fixtures, the [viewing guide](../docs/guide/07-viewing.md) (decks, and absent
   steps kept as placeholders in the run view too) and the viewer checks.

Evidence: unit tests of every rule, the §4 example included; a 500-node budget test; the
re-recorded conformance cases; the viewer fixture checks; a browser check of decks, expansion,
nested levels, reduced motion and live updates that move no cell.

**Ship B: the layout file.**

1. The `fx-layout-v1` reader, discovery, confinement and diagnostics in `crates/grida-fx-viewer`.
2. The report's file members on the route and the `layout` member of `inspect RUN --json`.
3. Record matrix `axes` (§2) in `crates/grida-fx-core` and `crates/grida-fx-runtime`, carry them
   onto fx-viewer-run-v1 nodes, edit the three schemas and re-record the matrix conformance case.
4. Grid spreads and matrix tables in `js/fx-web`.
5. The conformance identity case (§7); the spec gate validates committed `*.layout.json` files.
6. The guide, the installed skill and AR-17's evidence in [agent readiness](../AGENT_READINESS.md).

Evidence: the conformance identity case; reader tests for every diagnostic; repeats, nested scopes
and stale references in provider-free fixtures; recorded `axes` in the matrix case; a browser
check of grid spreads and an uneven matrix table.
