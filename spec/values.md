# Recorded canvas values

The viewer presents actual workflow data without adding executable constant/preview
steps. Authoring keeps `inputs`, `tables`, `let`, node `with` and workflow `outputs`,
through YAML or either SDK. Declared node defaults are available during planning,
including Python/TypeScript custom nodes. A fallback hidden inside a body is not a
declared default.

## Capture and ownership

Both graph families and run projections may contain `value_views` and `value_links`
arrays. Absence means no such evidence was recorded. The engine captures them during
ordinary binding/evaluation. The browser never evaluates expressions, rereads source,
compares values to infer provenance or parses display paths to reconstruct ownership.

A value view has `id` (`value:` followed by 64 lowercase hexadecimal characters, an opaque
portable identifier), `scope_id` (recorded owner or null
for root), `kind` (`input`, `table`, `let`, `output`), `name`, and `state` (`known`,
`pending`, `absent`, `unavailable`). Optional fields are `data` (encoded below), `context`
(portable occurrence label) and `diagnostic` (controlled presentation text).

Input views capture effective bound inputs, including defaults and omitted optional
inputs as genuine null. Saved-plan `inputs` retains its caller-supplied meaning and
identity effect. Tables are named raw data; strings inside tables are not expressions.

`let` is lazy and evaluated in the referencing environment. Items, matrices, takes and
judge/feedback contexts may change its value. Capture those occurrences with semantic
context, never an internal arena index. Do not evaluate unused declarations or expand
work merely to draw cards. Imported/repeated scopes retain recorded ownership.

Output views use normal output evaluation and its recorded source relationships.
Pending is distinct from absent, and known null is distinct from both. Presentation
failure/limits do not change evaluation results or fail executable work.

## Value encoding and availability

Known scalar/opaque JSON is `{value: <JSON>}`, including `{value: null}`. Typed files use
`{file: {digest, kind, name, size, key?}}`; `list` and `collection` use existing recorded
collection encoding. A typed object containing files uses `{object: {field: encoded_value}}`
recursively; this is presentation encoding only, not a change to execution encodings.
Arbitrary user JSON stays inside `value`, even when it has keys
such as `file`, `value`, `withheld` or `list`. Neither path-looking text nor saved-plan
`{file: digest}` establishes typed media.

Preserve null/missing/pending distinctions before legacy execution encoding loses them.
Do not reinterpret old `{none: true}` as a specific new state. Known file values may
have unavailable bytes: verified availability still comes from the artifact inventory.
No missing remote source is fetched to draw a card. A run may capture original bound input
bytes under `values/<digest>` ([store.md](store.md)); transformed outputs are not required
to preserve the original input's digest for its thumbnail to remain available.

Capture is bounded to 64 KiB per encoded value, encoded-data JSON depth 64, 4,096 value views, 16,384 links
and 8 MiB of value payload per snapshot. Bounds apply independently of canvas excerpts.
Oversized value evidence is explicitly unavailable and has no unsupported links; entries
beyond occurrence/link limits are omitted, never represented by fabricated connections.
This differs from a canvas excerpt, whose complete evidence may remain in details.

## Relationships

A link has `source` and `target`, each one of:

- `{kind: value, value_id}`;
- `{kind: instance, instance_id, port, source_kind?}`, with optional source kind `output`
  or `fact`.

An instance source requires `source_kind`; an instance target names a `with` field and
must not carry `source_kind`.

References address recorded values/ports, never renderer IDs. Links retain named sources
without replacing existing leaf/interface bindings. They report observed source
contributions, not invertible transformations, byte equality or consumption of every
expression branch. Unknown dynamic members create no guessed precise port links.
Plan-time evaluation does not prove execution: preserve the existing distinction
between planned relationships and reads by executed consumers. These links never
become scheduling dependencies.

## Records, resume and identity

Plans capture initial evidence; recorded expansion snapshots retain updates through
the same snapshot/replay path as scopes and notes. Inspection/resume consume captured
evidence, not edited source. Old records stay readable without guessed defaults,
provenance or historical states.

Presentation arrays do not enter executable plan digests, node/call identities,
scheduling, retries or cache lookup. Actual input/table/expression edits keep their
ordinary identity effects; notes' documentation exemption does not extend to real data.
Exact materialized-plan/export hashes still reflect the captured presentation.

## Canvas and details

Node cards show bounded effective parameters, including connected values and declared
defaults, plus recorded routing when available. Do not claim all effective fields were
explicitly authored or infer authored/default provenance by comparing values.

Inputs occupy the input side, shared sources sit near consumers and outputs occupy the
output side. Keep value placement separate from execution `cells` and member `order`.
Imported workflows follow the same rule without leaking their private interior into a
closed parent card. Contextual values follow existing scope/frame/deck ownership.

Grouped scalar rows retain individual sockets, activation targets and value references;
grouping creates no aggregate identity. Text is literal escaped text, not Markdown.
Objects/collections have bounded summaries and counts; details provide full inspection.
Reuse typed media previews, fullscreen and [external file activation](viewer.md#external-file-activation).
Measure text before layout and reserve media bounds before loading. Loading media
cannot resize the graph. New execution evidence may update layout while preserving
the user's camera. No authored-layout file is required.

## Disclosure and embedding

Exports apply existing value/file disclosure choices to all new positions. Withheld
known scalar/opaque JSON uses `state: withheld` with no `data`. Typed files retain only
existing sanitized metadata and permitted byte availability. Lists/collections retain
structure and nested withholding markers. Pending/absent/unavailable remain structural
states. Diagnostics require diagnostic disclosure; expression bodies, schema defaults
and nested values cannot bypass withholding through new metadata.

Value focus/reveal requires the independent `include_values` viewer opt-in (`includeValues`
in the browser SDK). Default clients see null when a value is selected. Messages contain
references and ownership, never contents, bytes or private paths. See [viewer.md](viewer.md).
