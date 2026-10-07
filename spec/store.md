# The store

The store is FX's cache. It keeps file bytes by content, step results by step identity, paid calls by call key, and the long provider jobs a run has submitted but not yet collected. It lives at the project's `cache` (default `.fx/cache`, set in `fx.yaml`), and every run of the project reads and writes it. The keys are defined in [identity.md](identity.md); the records' schemas are in [schemas/](schemas/). Each run also writes a run folder, which holds its record and links to the store's bytes (§8).

The project that owns the store and the run folders is the **planning project**: the folder of the nearest `fx.yaml` at or above the workflow file when the command names the workflow by the path of its `.yaml` file, and at or above the current directory otherwise (a workflow id, a builder). A workflow's own project (its home, where its `./` paths and node modules are found) may be another one, for example a workflow found by id in a nested project; its runs still use the planning project's `cache` and `runs`.

**Finding a workflow by id.** An id names the one workflow file whose `id` it is among the planning project's search files:
1. the `*.yaml` and `*.yml` files directly in the planning project's root, sorted by name;
2. then, for each folder of the project's `workflows` setting in its listed order (default `[workflows]`; fx-project-v1), the same files in that folder and in every folder below it, folders in the sorted order of their paths and each folder's files sorted by name. Symbolic links to folders are not followed, and a listed folder that does not exist adds nothing.

`fx.yaml` and a name containing `.takes.` are never search files, and a file found twice, through folders that overlap, counts once. A setting lists folders relative to the project root, or absolute ones, which may lie outside the project; an entry that is the project root or a folder above it is refused while the project file is read, since it would search the project's own runs and store. The setting replaces the default: a project that keeps workflows both in `workflows/` and elsewhere lists both. A workflow found this way has its home at the nearest `fx.yaml` at or above its file, as any other: its node modules, `./` paths, `fx.lock` and `sources` are its home's, and its home's route defaults (`routes`) apply under the planning project's. The planning project's `cache`, `runs`, `budget` and `route_tables` are used whatever the home says, and so is the planning project's root as the folder of the takes file when the home is another project.

The words MUST, MUST NOT and SHOULD are used as in RFC 2119.

## 1. Layout

```
.fx/cache/
  files/<d[:2]>/<d>            a file's bytes               d = file_digest
  results/<i[:2]>/<i>.json     fx-result-record-v1          i = step_identity
  calls/<k[:2]>/<k>.json       fx-call-record-v1            k = call_key
  jobs/<k>.json                fx-job-record-v1             k = the call_key it answers
  stand-in/                    the stand-in store: files/, results/ and calls/ as above, never jobs/ (§8)
```

- Every name is a digest: 64 lowercase hexadecimal characters ([identity.md](identity.md) §2). An engine MUST refuse to build a store path from anything else.
- Anything else under the store root, such as an engine's scratch space, is not part of this contract and MUST NOT be read as a record.
- `stand-in/` is a store of its own, used only by stand-in runs (§8, *Stand-in runs*). Its records are never read as this store's, nor this store's as its, and each keeps its own scratch space and leftovers.
- Records name files by digest only, so a store can be copied, moved or shared between projects and machines as it is.

## 2. Files

- A file is stored once, under its digest, and never changed. Its bytes SHOULD be written read-only, because a run folder may link to them.
- A file's name, kind and key are not stored with its bytes. The records that name it carry them.
- A file is **present** when `files/<d[:2]>/<d>` exists and has the size its record states. Trusting a record (§4) checks presence only; a full check rehashes the bytes.

## 3. Records

Every record is a JSON object whose `kind` names its schema. It is written as `canon(record)` ([identity.md](identity.md) §2), so two engines write the same bytes for the same record. Money follows [identity.md](identity.md) §12.

| Record | What it holds |
|---|---|
| result | What one step identity produced: its `outputs` by port, the `facts` the node reported, its `read` set, and `cost_usd`: what the run paid for the step's calls, null when it paid for none (calls answered from the cache cost nothing). |
| call | One paid call's answer: the `files` and `data` the provider returned and `cost_usd` (null when the provider reported none), with the `capability`, `route` (id and fingerprint), canonical `request` and `take` it is keyed by. |
| job | A long provider job of one call that is not answered yet: its `state` and `handle`, with the same key fields as the call record. |

**Outputs** in a result record are encoded as:
- a file: `{"file": {"digest", "kind", "name", "size", "key"?}}`, with `key` only for a keyed collection's item;
- a keyed collection: `{"collection": [[key, output], …]}`, in collection order;
- a list: `{"list": [output, …]}`;
- nothing: `{"none": true}`.

Facts that the engine computes from bytes ([identity.md](identity.md) §4) are not stored; they are computed again from the file.

The **read set** of an instance maps `<name>/<index>` to a file digest: for each `with:` name, every file its value holds, in order, numbered from 0.

A call record is enough to recompute its own key: `call_key` over its `capability`, `route.fingerprint`, `request` and `take` ([identity.md](identity.md) §9) MUST equal its `key`. An engine MUST NOT add anything to `request` that it did not hash, or remove anything from it.

## 4. When a record is trusted

A record that fails a check below is **absent**. The engine does the work again, as if there were no record. An absent record is never an error, and never partly used.

A **result record** is trusted only when:
1. it parses, its `kind` is `fx-result-record-v1`, and its `identity` is the identity it was looked up under;
2. its `read` equals the read set of the instance asking, exactly;
3. every file in its `outputs` is present.

A **call record** is trusted only when:
1. it parses, its `kind` is `fx-call-record-v1`, and its `key` is the key it was looked up under;
2. every file in its `files` is present.

A trusted call record answers the call before anything that could send it, and bills nothing. Recorded calls therefore replay offline.

A **job record** that cannot be read is an error, not an absence, because it may stand for a paid submission. The run stops and names the record.

## 5. Long jobs

A capability whose provider job outlives one request (a video, a rig) keeps a job record under the call's key.

| `state` | Written | What the next run does |
|---|---|---|
| `submitting` | before the submit request may leave; `handle` is null | Stops for a person: nobody can say whether the provider took the job, or billed it. `grida-fx jobs` lists such jobs and forgets one once a person has checked the provider. |
| `submitted` | once the provider acknowledged the job, with the `handle` that collecting needs | Collects the job by its handle, and never submits it again. Collecting bills nothing new: the run that submitted it was charged its hold. |
| `settled` | when the job ended without a result and nothing of it is outstanding, such as a job the provider reported failed | Submits the call anew, as a new attempt. |

A `submitting` record MAY carry `note`: when the submit's outcome is unknown, the redacted reason, which names the provider's job id when one was returned, so a person can find the job. No other record carries one. `grida-fx jobs` shows it, and a later run's `job_unsettled` carries it as `reason` in `data`.

Once the call is answered, its call record is published and then its job record is removed. A trusted call record also removes a leftover job record with its key. A `settled` record MAY be removed at any time.

## 6. Writing

- **Atomic publish.** Every file and record MUST be written to a temporary name in its destination folder (a name that is never a digest), flushed to disk, and then renamed over its final name. A reader sees the whole content or nothing.
- **Bytes first.** A record is published only after every file it names: a call record after its files, a result record after its output files. A job record is removed only after the call record is published. A crash therefore never leaves a record whose bytes are missing.
- **No rewrites.** A file that is present is not written again.
- **Concurrent writers.** Two writers of one name each publish a whole record; the last rename wins, and either is a valid record.
- **Leftovers.** A writer that stopped (a killed run) may leave a temporary file behind. A temporary name is never read as a record, and a run removes the temporary files it finds in the store once they are an hour old, since one that is still being written is changed far more often. An engine that keeps scratch space under the store root (work dirs) removes what an invocation that no longer runs left there.

## 7. What a record never holds

- **No secrets:** no keys, authorization headers, cookies, tokens or signed URLs. Requests never carry them, because credentials travel in the transport ([identity.md](identity.md) §9). A job's `handle` holds only what collecting needs, such as the provider's job id. A file a provider returns at a URL is downloaded and stored, and its URL is not kept in `data`.
- **No paths:** no absolute paths, temporary paths or host names. Files are named by digest only.

## 8. Run folders

A run keeps its record in a run folder, and links its results there from the store. The folder is for people and for the commands that read a run (`inspect`, `project`, `reroll`, `pick`). The store stays the cache: deleting a run folder loses nothing the store holds, and a copied run folder can be read on another machine.

**Where.** `grida-fx run --run <folder>` runs in `<folder>`, relative to the current directory. Without `--run`, the run gets a new folder:

```
<runs>/<workflow id>/<YYYY-MM-DD>-<n>/
```

- `<runs>` is the planning project's `runs` folder from `fx.yaml` (default `runs`), relative to its root.
- `<YYYY-MM-DD>` is the local date when the command starts.
- `<n>` is the smallest integer from 1 for which nothing of that name exists yet. The engine claims the name by making the folder, an operation that fails when the name exists: an invocation that finds it taken meanwhile tries the next `n`, so invocations starting at once never share a new folder. A run refused before it wrote anything removes the folder again.

`grida-fx inspect` also takes a workflow id in place of a folder: that workflow's newest run under `<runs>/<workflow id>/`, the folder whose `plan.json` was written last.

**Resuming.** Running in a folder that already holds a run continues it. Finished steps come back from the record and the store, and answered calls replay without being billed. A folder whose `plan.json` records a different plan digest ([identity.md](identity.md) §10), or whose `events.jsonl` holds an event with another `plan`, is refused before anything runs, with the advice to choose a new folder; so is a folder of the other mode, a stand-in run resumed without a stand-in or the reverse (*Stand-in runs* below). The engine reads both only once it holds `run.lock`, so no other invocation writes the folder between the check and the run.

**Layout.**

```
<run folder>/
  plan.json                       fx-graph-v1        the plan the run started from
  events.jsonl                    fx-run-events-v1   everything that happened, in order
  run.lock                                           locked by the invocation running the folder
  files/<step>/<port><suffix>                        each step's output files
  outputs/<name><suffix>                             the workflow's declared outputs
```

- **`plan.json`** is the fx-graph-v1 document that `grida-fx expand` prints, `types` included, with `plan` (the plan digest), `steps`, `inputs` and `view_origins` added, and `takes_file`: the POSIX path of the workflow's takes file relative to the planning project's root, which `reroll` and `pick` write to. A stand-in run's also has `"stand_in": true`; no other run's has the member. The first invocation writes it before any step runs, under a temporary name and then renamed (§6). Later invocations compare its `plan` and never rewrite it.
- **`events.jsonl`** is the record, and the source of truth for every command that reads the run. Each line is one fx-run-events-v1 event, written as `canon(event)` and then one line feed (U+000A), oldest first. Each line is written whole and flushed before the run goes on, in one write. A line that cannot be written whole (a full disk can take part of one) MUST NOT stay: the engine cuts the file back to its length before the line, writes nothing more if it cannot, and stops the run, so no line ever follows part of a line. A last line without its line feed is a line an invocation began and never finished (it was killed): readers leave it out, and the next invocation cuts it off, says so, and appends after it. A resumed run appends lines under a new `invocation_id`. Lines are never rewritten or removed otherwise. Every line reads back as JSON ([identity.md](identity.md) §2, nested at most 512 deep): a node fact or mark nested deeper than 509 fails its node ([protocol.md](protocol.md) §5.3), and an encoded list (fx-run-events-v1 `encoded`) nested so deep that its `{"list": …}` levels would pass that is written once as `{"value": <its plain JSON>}`, which reads back as the same list.
- **`run.lock`** holds an exclusive operating-system file lock (`flock` on POSIX), taken without waiting, for as long as an invocation runs the folder. A second invocation that cannot take the lock is refused at once ("another invocation is running <folder>"). The lock ends with the process, so a crashed run leaves no stale lock. The file's content means nothing, and the file stays in place.
- **`files/`** gets an instance's output files when it succeeds, whether it ran or came from the cache, before its `node_finished` event is written. Each take of a step has a folder of its own (`<step>` below), so `files/` holds every take's files and `inspect --verify` checks each against its record.
- **`outputs/`** gets the workflow's declared outputs at the end of each invocation, before `run_finished`. An incomplete run places the outputs that exist. An output that holds no file, such as a plain value, has no entry here; its value is in the `run_finished` event.
- `views/` is reserved for views, which are planned and not part of this version.

**Display scopes and workflow interfaces.** New expanded graphs optionally record
`scopes`, and each instance records `interface_bindings`, separate from flattened
`bindings`. These fields describe authored boundaries; they change no value, read set,
identity, price, cache lookup or execution decision. A scope is one expanded inline
`group` or imported `workflow` occurrence. Its stable display `id` is `scope:` followed
by its instance `path`, `#`, and its enclosing group take numbers joined by `.`; `take`
contains those numbers (possibly empty). Repeat keys remain in `path`. `parent` is its
containing scope's id, or null at the root; `step` is the declaration path. `title` prefers
the authored step title, then an imported workflow's title, then the step name. `source`
is the imported workflow's portable loaded source, or null for a group. Scope ids are
unique; parents form an acyclic forest. `nodes` lists direct leaf instance ids and
`pending` lists direct pending repeat paths. Nested members belong to their direct
scope only. Unexpanded repeats do not acquire hypothetical scopes from shadow pricing.

`ports.inputs` preserves the workflow's input declarations; `ports.outputs` lists its
output alias names. Workflow outputs have no declared node-port type: readers MUST NOT
invent one. Inline groups have empty boundary ports. `input_bindings` records the
observed source of each supplied workflow input; `output_bindings` records observed
sources of its output aliases. Each uses `{source, source_port, source_kind, target_port}`.
For `output` or `fact`, the source is a leaf instance. For `scope_input` or `scope_output`,
it is an explicit scope boundary and its named declared port. `target_port` is the input
or output alias in that scope; in an instance's `interface_bindings` it is its input or
parameter. Node references preserve their actual take; boundary references preserve the
actual selected public alias, including two aliases that wrap the same inner output.
A scope input referenced inside its workflow remains that boundary input even when its
caller supplied a literal. Bindings are display references, not claims of value equality
or passthrough: an expression can compute from several sources. Unread output aliases,
unresolved selection and ambiguous per-item attribution have no fabricated mapping.
All observations are made during existing expression evaluation, never by evaluating
more code or opening source from a viewer.

A run records `scopes_updated {scopes, node_interface_bindings}` when this display
snapshot changes, before events for affected dynamically created nodes, and after the
final expansion. The per-node object maps instance ids to authoritative binding arrays;
empty arrays clear earlier bindings. The snapshot is complete for the current expansion,
not a patch. A reader keeps node execution history independently, applies the latest
snapshot, and can display historical nodes no longer belonging to a current scope at
root. Replays ignore this display-only event. Old records without these fields remain
flat; readers MUST NOT infer imported workflow boundaries or aliases from path strings,
artifact digests or the filesystem. A saved-plan host refuses invalid, cyclic or dangling scope metadata. A run viewer omits
such metadata with a warning, without hiding the underlying execution record.

**Named connection metadata.** New expanded graphs include `types.<uses>.ports`, with
`inputs` and `outputs` mapped to their declared port notation and `params` mapped to
their JSON Schemas ([protocol.md](protocol.md) §4). Each instance also includes
`bindings`: deduplicated evaluated references of
`{source, source_port, target_port, source_kind}`. `source` is the exact upstream
instance id, `source_kind` is `output` or `fact`, and `target_port` names the
destination's `with` entry, either a file input or a parameter. A compound expression
or collection can have several bindings to one entry. File facts keep the original
file output as their source; node facts use `source_kind: fact`.

Bindings describe references evaluated while resolving that entry, rather than a
claim that the entry equals the upstream value. They do not include short-circuited
operands, ordering dependencies, or reads leaked from another step's expansion.
An unresolved choice of repeat key or take has no invented port binding; its ordinary
dependency remains visible. Runtime re-expansion supplies exact bindings when it can.
`node_started` records `ports`, `bindings`, `needs`, and `judges` for the instance,
including instances absent from the initial plan. A reader uses these later fields
in preference to the initial graph, including an explicitly empty binding list.
Failures and blocked skips before dispatch also record the known instance's
declarations and bindings on their terminal event; they do not invent a start event.
For a repeat item derived from one resolved output, the engine can forward that
source into the item's parameters. When several origins make that attribution
ambiguous, it retains ordinary dependencies rather than claiming an exact port wire.

All these fields are display metadata, outside node identities, call keys, and the
plan digest. They do not affect scheduling or values. They are optional when reading
older graphs and events: absence means that named wiring was not recorded, and a
viewer may show ordinary dependency lines without inferring port names from digests
or resolved values.

**Placing a file.** A file in `files/` or `outputs/` is a hard link to the store's copy (§2), or a copy of it where linking fails, for example across file systems. It is placed like a store write (§6): written under a temporary name in its destination folder, then renamed over its final name. The temporary name is `.<16 lowercase hex>.part`, 22 bytes, so it fits wherever the final name fits. A name that already holds the same bytes is left alone, so a resumed run does not place it again. Any other file there is replaced. Placed files share bytes with the store, so nothing may edit them in place. An invocation that holds `run.lock` removes the temporary names an earlier, killed invocation left in the folder.

**Names.**
- `<step>` is the instance's step path, repeat keys included, as one folder name:
  - every character that is not a letter or a digit (Unicode general category L or N), `.`, `_` or `-` becomes `_`;
  - leading and trailing `_` are then removed;
  - an empty result becomes `step`.

  So `entity['ada'].draw` becomes `entity__ada__.draw`. When the instance's take list is not `[1]` (a regenerated step, or one with several takes), `#` and its takes joined by `.` follow: `draw#2`, `entity__ada__.draw#1.3`.
- `<port>` is the output port's name, and `<name>` the declared output's name.
- `<suffix>` is the first suffix that [identity.md](identity.md) §4 lists for the file's kind: `.jpg` for `image/jpeg`, `.json` for `json`. The kind `annotations` takes `.json`. Every other kind, `file` included, takes no suffix. The suffix is not added again when the name already ends with it, compared case-sensitively: a key `hero.png` gives `hero.png`, not `hero.png.png`.

**One file or several.** A value's files are taken in order: a list's items, a keyed collection's items in collection order, and an object's members in order.
- **Step files.** A port that holds exactly one file gives `files/<step>/<port><suffix>`. A port that holds several gives `files/<step>/<port>/<label><suffix>` for each.
- **Outputs.** An output whose value is one file, and not a list or a keyed collection, gives `outputs/<name><suffix>`. Any other output gives `outputs/<name>/<label><suffix>` for each of its files, even when there is only one.
- **Labels.** A file's `<label>` is its key when it has a non-empty one: the key of its item in a keyed collection, whether a node's keyed output or a repeat's instances (the key text, [identity.md](identity.md) §11). Otherwise it is the file's position among the value's files, counted from 0.
- **Keys as paths.** A key is split at `/`. Each segment that is empty, `.` or `..` becomes `_`. Every other segment is converted as `<step>` is. The segments are joined again with `/`. A key with `/` therefore makes folders, and no key leaves `<port>/` or `<name>/`.

- **Length.** Each segment of a placed path (`<step>`, `<port>`, `<name>`, each key segment, the file name with its suffix) holds at most 255 bytes of UTF-8, what file systems take for one name. A longer segment keeps its first bytes (whole characters), followed by `~`, the first 16 lowercase hex characters of the SHA-256 of the whole segment, and, for the file name, the suffix it ended with, 255 bytes in all. `~` never appears in a converted segment, so a cut name is never another segment's whole name.

These names are for reading, and they are not identities. Two step paths or keys can map to one name: `a b` and `a_b`, or two names that differ only in letter case on a case-insensitive file system. Only one of the files then sits at that name. `events.jsonl` names every file by digest.

**JSON in a run folder.** A node's JSON output is a file that the engine writes in the format of [identity.md](identity.md) §5, "Writing JSON". It is stored under its digest, and the run folder links to those bytes. `plan.json` uses the same format. Records use `canon(record)`: the store's records (§3) and each line of `events.jsonl`.

**Nothing private.** `plan.json` and `events.jsonl` follow §7, the same as a record: no secrets, and no absolute or temporary paths. Files are named by digest, and file paths are relative to the project.

**Stand-in runs.** A stand-in run ([protocol.md](protocol.md) §5.7) stores everything in the **stand-in store**, `stand-in/` under the planning project's store root, with the layout of §1 and no `jobs/`. Its run folder is placed and named as any other's.
- **Nothing reaches the store.** The run's input files, call records, step results and output files go to the stand-in store and only there. A stand-in's answer, or a result made from one, never becomes a record of the store, since a step's identity does not depend on how its calls were answered.
- **Nothing comes from the store.** A stand-in run trusts no record of the store, call or result: every lookup of §4 is the stand-in store's. What a stand-in is asked therefore never depends on what earlier paid runs left in the cache, and no paid answer hides a stand-in's checks.
- **Answers are kept.** A stand-in's answers and the results made from them are records of the stand-in store like any other, so a resumed or later stand-in run of the project replays them, whatever its stand-in. Removing `stand-in/` forgets them.
- **Marked.** The run's `plan.json` holds `"stand_in": true`, and each `run_started` event `stand_in: true`. The mode is not part of the plan digest ([identity.md](identity.md) §10): a stand-in run and a run without one have the same digest for the same plan.
- **One mode per folder.** Resuming a folder in the other mode is refused before anything runs: `<folder> holds a stand-in run; resume it with --stand-in, or choose a new folder`, and `<folder> holds a run without a stand-in; resume it without --stand-in, or choose a new folder`. A folder holds a stand-in run when its `plan.json` says `"stand_in": true`.
- **The takes file is not touched.** `reroll` and `pick` refuse a stand-in run: `<folder> holds a stand-in run, and stand-in runs never change the workflow's takes file`. A stand-in run, and every command that reads one, writes nothing outside its run folder and the stand-in store, apart from what `--deliver` copies out.
- **Read like any run.** `inspect` and `project` read a stand-in run's folder as any other's, and say that it is one. An SDK that reads a run's files by digest reads a stand-in run's from the stand-in store.

## 9. Changes from stage-gen's engine

FX comes from the Python engine stage-gen ran on until it moved onto FX. That engine's store and run folders differed as follows.

| stage-gen | FX |
|---|---|
| The store sits in a folder named after the old engine, and its records carry the old engine's kinds. | `.fx/cache`, with `fx-result-record-v1`, `fx-call-record-v1` and `fx-job-record-v1`. An old record is absent to FX, and none is migrated: it keeps no request (next row), so its FX key cannot be computed from it. A project that moves to FX starts with an empty cache. |
| A call record keeps only `kind`, `key`, `files`, `data` and `cost_usd`. | It also keeps `capability`, `route`, `request` and `take`, so its key can be recomputed (§3). |
| A job record's `route` is an id and its `take` an integer, and it keeps no request. | `route` is `{id, fingerprint}`, `take` is a list, and the request is kept. |
| A settled job's record is deleted. | It MAY be kept, as `settled`. |
| Records are written with Python's JSON encoder. | `canon(record)`, RFC 8785. |
| Event lines are Python's compact JSON with sorted keys, and carry `schema_version` and `graph_sha256`. Cancelling writes `run_canceled`. | `canon(event)`. `kind` carries the version, `plan` holds the plan digest, and the event is `run_cancelled` (§8). |
| `plan.json` is written with Python's JSON encoder and a trailing newline. After the run it is rewritten to add the absolute paths of the workflow and the project, a builder's arguments, and the input values. | fx-graph-v1, in the identity.md §5 "Writing JSON" format. It is written once and holds no absolute path (§8). It records the takes file by its project-relative path (`takes_file`), so `reroll` and `pick` find it without a path to the workflow. |
| A run folder's suffix table has no entry for `image/gif`, `audio/ogg`, `text/yaml`, `text/toml`, `text/html` or `model/gltf+json`, so those files are placed without a suffix. | The first suffix that identity.md §4 lists for the kind (§8). |
| A file is placed only when its name holds nothing or a file of another size, so a different file of the same size, such as a later take, is never placed. | A name is left alone only when it already holds the same bytes (§8). |
| Every take of a step is placed at `files/<step>/`, so the folder holds one take, and `inspect --verify` fails on a healthy run with several takes. | Each take has its own folder, `<step>#<takes>` when its take list is not `[1]` (§8). |
| A name longer than a file system takes, or a temporary name longer than the final one, cannot be placed, and the run never completes. | Segments are cut to 255 bytes with a digest, and temporary names are 22 bytes (§8). |
| Two invocations started at once may pick the same new folder. | A new folder is claimed by making it (§8). |
| A failed event write can leave part of a line, followed by later lines, and the folder can no longer be resumed. | A line is written whole or not at all, and a torn last line is cut off by the next invocation (§8). |
| What a killed run leaves (work dirs, temporary files) stays. | A later run removes it (§6, §8). |
