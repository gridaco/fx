# The store

The store is FX's cache. It keeps file bytes by content, step results by step identity, paid calls by call key, and the long provider jobs a run has submitted but not yet collected. It lives at the project's `cache` (default `.fx/cache`, set in `fx.yaml`), and every run of the project reads and writes it. The keys are defined in [identity.md](identity.md); the records' schemas are in [schemas/](schemas/). Each run also writes a run folder, which holds its record and links to the store's bytes (§8).

The words MUST, MUST NOT and SHOULD are used as in RFC 2119.

## 1. Layout

```
.fx/cache/
  files/<d[:2]>/<d>            a file's bytes               d = file_digest
  results/<i[:2]>/<i>.json     fx-result-record-v1          i = step_identity
  calls/<k[:2]>/<k>.json       fx-call-record-v1            k = call_key
  jobs/<k>.json                fx-job-record-v1             k = the call_key it answers
```

- Every name is a digest: 64 lowercase hexadecimal characters ([identity.md](identity.md) §2). An engine MUST refuse to build a store path from anything else.
- Anything else under the store root, such as an engine's scratch space, is not part of this contract and MUST NOT be read as a record.
- Records name files by digest only, so a store can be copied, moved or shared between projects and machines as it is.

## 2. Files

- A file is stored once, under its digest, and never changed. Its bytes SHOULD be written read-only, because a run folder may link to them.
- A file's name, kind and key are not stored with its bytes. The records that name it carry them.
- A file is **present** when `files/<d[:2]>/<d>` exists and has the size its record states. Trusting a record (§4) checks presence only; a full check rehashes the bytes.

## 3. Records

Every record is a JSON object whose `kind` names its schema. It is written as `canon(record)` ([identity.md](identity.md) §2), so two engines write the same bytes for the same record. Money follows [identity.md](identity.md) §12.

| Record | What it holds |
|---|---|
| result | What one step identity produced: its `outputs` by port, the `facts` the node reported, its `read` set, and `cost_usd` (null when the step made no paid call). |
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

Once the call is answered, its call record is published and then its job record is removed. A trusted call record also removes a leftover job record with its key. A `settled` record MAY be removed at any time.

## 6. Writing

- **Atomic publish.** Every file and record MUST be written to a temporary name in its destination folder (a name that is never a digest), flushed to disk, and then renamed over its final name. A reader sees the whole content or nothing.
- **Bytes first.** A record is published only after every file it names: a call record after its files, a result record after its output files. A job record is removed only after the call record is published. A crash therefore never leaves a record whose bytes are missing.
- **No rewrites.** A file that is present is not written again.
- **Concurrent writers.** Two writers of one name each publish a whole record; the last rename wins, and either is a valid record.

## 7. What a record never holds

- **No secrets:** no keys, authorization headers, cookies, tokens or signed URLs. Requests never carry them, because credentials travel in the transport ([identity.md](identity.md) §9). A job's `handle` holds only what collecting needs, such as the provider's job id. A file a provider returns at a URL is downloaded and stored, and its URL is not kept in `data`.
- **No paths:** no absolute paths, temporary paths or host names. Files are named by digest only.

## 8. Run folders

A run keeps its record in a run folder, and links its results there from the store. The folder is for people and for the commands that read a run (`inspect`, `project`, `reroll`, `pick`). The store stays the cache: deleting a run folder loses nothing the store holds, and a copied run folder can be read on another machine.

**Where.** `grida-fx run --run <folder>` runs in `<folder>`, relative to the current directory. Without `--run`, the run gets a new folder:

```
<runs>/<workflow id>/<YYYY-MM-DD>-<n>/
```

- `<runs>` is the project's `runs` folder from `fx.yaml` (default `runs`), relative to the project root.
- `<YYYY-MM-DD>` is the local date when the command starts.
- `<n>` is the smallest integer from 1 for which nothing of that name exists yet.

`grida-fx inspect` also takes a workflow id in place of a folder: that workflow's newest run under `<runs>/<workflow id>/`, the folder whose `plan.json` was written last.

**Resuming.** Running in a folder that already holds a run continues it. Finished steps come back from the record and the store, and answered calls replay without being billed. A folder whose `plan.json` records a different plan digest ([identity.md](identity.md) §10) is refused before anything runs, with the advice to choose a new folder.

**Layout.**

```
<run folder>/
  plan.json                       fx-graph-v1        the plan the run started from
  events.jsonl                    fx-run-events-v1   everything that happened, in order
  run.lock                                           locked by the invocation running the folder
  files/<step>/<port><suffix>                        each step's output files
  outputs/<name><suffix>                             the workflow's declared outputs
```

- **`plan.json`** is the fx-graph-v1 document that `grida-fx expand` prints, `types` included, with `plan` (the plan digest), `steps`, `inputs` and `view_origins` added. The first invocation writes it before any step runs, under a temporary name and then renamed (§6). Later invocations compare its `plan` and never rewrite it.
- **`events.jsonl`** is the record, and the source of truth for every command that reads the run. Each line is one fx-run-events-v1 event, written as `canon(event)` and then one line feed (U+000A), oldest first. Each line is written whole and flushed before the run goes on. A resumed run appends lines under a new `invocation_id`. Lines are never rewritten or removed.
- **`run.lock`** holds an exclusive operating-system file lock (`flock` on POSIX), taken without waiting, for as long as an invocation runs the folder. A second invocation that cannot take the lock is refused at once ("another invocation is running <folder>"). The lock ends with the process, so a crashed run leaves no stale lock. The file's content means nothing, and the file stays in place.
- **`files/`** gets an instance's output files when it succeeds, whether it ran or came from the cache, before its `node_finished` event is written.
- **`outputs/`** gets the workflow's declared outputs at the end of each invocation, before `run_finished`. An incomplete run places the outputs that exist. An output that holds no file, such as a plain value, has no entry here; its value is in the `run_finished` event.
- `views/` is reserved for views, which are planned and not part of this version.

**Placing a file.** A file in `files/` or `outputs/` is a hard link to the store's copy (§2), or a copy of it where linking fails, for example across file systems. It is placed like a store write (§6): written under a temporary name in its destination folder, then renamed over its final name. A name that already holds the same bytes is left alone, so a resumed run does not place it again. Any other file there is replaced. Placed files share bytes with the store, so nothing may edit them in place.

**Names.**
- `<step>` is the instance's step path, repeat keys included, as one folder name:
  - every character that is not a letter or a digit (Unicode general category L or N), `.`, `_` or `-` becomes `_`;
  - leading and trailing `_` are then removed;
  - an empty result becomes `step`.

  So `entity['ada'].draw` becomes `entity__ada__.draw`.
- `<port>` is the output port's name, and `<name>` the declared output's name.
- `<suffix>` is the first suffix that [identity.md](identity.md) §4 lists for the file's kind: `.jpg` for `image/jpeg`, `.json` for `json`. The kind `annotations` takes `.json`. Every other kind, `file` included, takes no suffix. The suffix is not added again when the name already ends with it, compared case-sensitively: a key `hero.png` gives `hero.png`, not `hero.png.png`.

**One file or several.** A value's files are taken in order: a list's items, a keyed collection's items in collection order, and an object's members in order.
- **Step files.** A port that holds exactly one file gives `files/<step>/<port><suffix>`. A port that holds several gives `files/<step>/<port>/<label><suffix>` for each.
- **Outputs.** An output whose value is one file, and not a list or a keyed collection, gives `outputs/<name><suffix>`. Any other output gives `outputs/<name>/<label><suffix>` for each of its files, even when there is only one.
- **Labels.** A file's `<label>` is its key when it has a non-empty one: the key of its item in a keyed collection, whether a node's keyed output or a repeat's instances (the key text, [identity.md](identity.md) §11). Otherwise it is the file's position among the value's files, counted from 0.
- **Keys as paths.** A key is split at `/`. Each segment that is empty, `.` or `..` becomes `_`. Every other segment is converted as `<step>` is. The segments are joined again with `/`. A key with `/` therefore makes folders, and no key leaves `<port>/` or `<name>/`.

These names are for reading, and they are not identities. Two step paths or keys can map to one name: `a b` and `a_b`, or two names that differ only in letter case on a case-insensitive file system. Only one of the files then sits at that name. `events.jsonl` names every file by digest.

**JSON in a run folder.** A node's JSON output is a file that the engine writes in the format of [identity.md](identity.md) §5, "Writing JSON". It is stored under its digest, and the run folder links to those bytes. `plan.json` uses the same format. Records use `canon(record)`: the store's records (§3) and each line of `events.jsonl`.

**Nothing private.** `plan.json` and `events.jsonl` follow §7, the same as a record: no secrets, and no absolute or temporary paths. Files are named by digest, and file paths are relative to the project.

## 9. Changes from stage-gen's engine

FX comes from the Python engine in stage-gen. Its store and run folders differ as follows.

| stage-gen | FX |
|---|---|
| The store sits in a folder named after the old engine, and its records carry the old engine's kinds. | `.fx/cache`, with `fx-result-record-v1`, `fx-call-record-v1` and `fx-job-record-v1`. An old record is absent to FX; milestone 2 migrates the paid cache by replay. |
| A call record keeps only `kind`, `key`, `files`, `data` and `cost_usd`. | It also keeps `capability`, `route`, `request` and `take`, so its key can be recomputed (§3). |
| A job record's `route` is an id and its `take` an integer, and it keeps no request. | `route` is `{id, fingerprint}`, `take` is a list, and the request is kept. |
| A settled job's record is deleted. | It MAY be kept, as `settled`. |
| Records are written with Python's JSON encoder. | `canon(record)`, RFC 8785. |
| Event lines are Python's compact JSON with sorted keys, and carry `schema_version` and `graph_sha256`. Cancelling writes `run_canceled`. | `canon(event)`. `kind` carries the version, `plan` holds the plan digest, and the event is `run_cancelled` (§8). |
| `plan.json` is written with Python's JSON encoder and a trailing newline. After the run it is rewritten to add the absolute paths of the workflow and the project, a builder's arguments, and the input values. | fx-graph-v1, in the identity.md §5 "Writing JSON" format. It is written once and holds no absolute path (§8). |
| A run folder's suffix table has no entry for `image/gif`, `audio/ogg`, `text/yaml`, `text/toml`, `text/html` or `model/gltf+json`, so those files are placed without a suffix. | The first suffix that identity.md §4 lists for the kind (§8). |
| A file is placed only when its name holds nothing or a file of another size, so a different file of the same size, such as a later take, is never placed. | A name is left alone only when it already holds the same bytes (§8). |
