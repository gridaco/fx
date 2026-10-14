# Identity

Every cache in FX is keyed by a content identity. This document defines each one exactly, so that any implementation (the Rust engine, the independent checker in `tools/digest.py`, a future one) computes the same bytes and the same digests. A change here is a change to every cache: it needs a new `kind` version and a migration note. The one exception is the route contract's completion (§7), which kept `fx-route-v1` because it moved no built-in fingerprint.

The words MUST, MUST NOT and SHOULD are used as in RFC 2119.

## 1. Values

FX values are JSON values with three restrictions, the I-JSON profile ([RFC 7493](https://www.rfc-editor.org/rfc/rfc7493)):

- **Numbers** are IEEE 754 binary64. There is one number type: `1` and `1.0` are the same value. NaN and ±infinity are not values. An integer literal (one with no fraction or exponent) that cannot be read without rounding is refused where it is read: the literal must be the canonical form (§2) of the number it reads as, apart from the sign of zero. `9007199254740993` and `12345678901234567891` are refused; `9007199254740992` and `10000000000000000` are exact and accepted, so FX always reads back what it writes.
- **Strings** are sequences of Unicode scalar values. A lone surrogate is refused. Text is never normalized (no NFC, no case folding).
- **Object keys** are strings and unique within an object.

Every reader of documents (the strict YAML loader in [yaml.md](yaml.md), JSON inputs, JSON file content, the node protocol) MUST refuse input outside this domain rather than coerce it.

## 2. Canonical JSON and digests

- `canon(v)` is the JSON Canonicalization Scheme, [RFC 8785](https://www.rfc-editor.org/rfc/rfc8785) (JCS), encoded as UTF-8. In brief: no whitespace; object keys sorted by their UTF-16 code units; strings escaped minimally; numbers serialized as ECMAScript `Number.prototype.toString` does (`1`, `0.5`, `1e+21`, `1e-7`). Implementations MUST pass the vectors in `vectors/jcs/`.
- `digest(v)` is the SHA-256 of `canon(v)`, written as 64 lowercase hexadecimal characters with no prefix.
- `file_digest(f)` is the SHA-256 of the file's raw bytes, in the same form. Bytes are never normalized: line endings, a byte-order mark and encoding are all part of the content.

Every object hashed in this document carries a `kind` member naming the formula and its version (`"fx-step-v1"`, …). Two formulas can never produce colliding inputs, and a formula can change by bumping its version.

## 3. The plain projection

Identity hashes values through `plain(v)`, which turns the engine's runtime values into JSON values:

| Runtime value | `plain(v)` |
|---|---|
| a JSON value | itself; objects and arrays recursively |
| a file | `{"file": file_digest}`. Its name, kind, key and size are not part of it. |
| a missing value (the output of a step that is absent, skipped or rejected) | `{"missing": true}` |
| a failed upstream result | `{"failed": instance_id}` |
| a keyed collection | `{"collection": [[key, plain(item)], …]}` in collection order |
| a pending value (not known until something runs) | none: any identity that would contain it is `null` |

A declared optional workflow input that is not given is `null`, an ordinary JSON value.

The projection must be injective, so user data cannot look like a runtime marker. A JSON value read from a workflow, an inputs file, a table or a JSON file is refused where it is read if it contains an object whose only member is one of these, in this shape:
- `file` with a 64-character lowercase hex string;
- `missing` with `true`;
- `failed` with a string;
- `collection` with an array;
- `pending` with any value.

An object like `{"file": "a.png"}` or `{"missing": false}` is ordinary data.

## 4. File kinds and file facts

A file's **kind** comes from its suffix, compared case-insensitively. It is not part of a file's plain projection (§3). It enters an identity only as a file fact that an expression reads.

| Suffix | Kind | Suffix | Kind |
|---|---|---|---|
| `.png` | `image/png` | `.wav` | `audio/wav` |
| `.jpg`, `.jpeg` | `image/jpeg` | `.mp3` | `audio/mpeg` |
| `.webp` | `image/webp` | `.ogg` | `audio/ogg` |
| `.gif` | `image/gif` | `.mp4` | `video/mp4` |
| `.md` | `text/markdown` | `.webm` | `video/webm` |
| `.txt` | `text/plain` | `.mkv` | `video/x-matroska` |
| `.json` | `json` | `.glb` | `model/gltf-binary` |
| `.yaml`, `.yml` | `text/yaml` | `.gltf` | `model/gltf+json` |
| `.toml` | `text/toml` | `.fbx` | `model/fbx` |
| `.html` | `text/html` | `.zip` | `file/zip` |
| anything else | `file` | | |

**File facts** are values the engine computes from a file's bytes. Node hosts never compute them. (A node body may also report its own values with `fact()`; those are *node facts*, part of the node's result.) [facts.md](facts.md) defines every file fact, images included; the table above stays normative here, and the rest of this section is an informative summary. `facts(f)` always has `bytes` (the size) and `kind`. Images (`image/png`, `image/jpeg`, `image/webp`, `image/gif`) add:

- `width`, `height`: pixels of the first frame;
- `has_alpha`: true when the encoding can carry transparency: an alpha channel, a PNG `tRNS` chunk, a WebP alpha flag, or a GIF transparent index;
- `opaque`: true when `has_alpha` is false or every pixel of the first frame is fully opaque.

WAV audio (`audio/wav`) adds `duration`; MP4 (`video/mp4`) and Matroska or WebM video (`video/x-matroska`, `video/webm`) add `width`, `height`, `fps`, `duration`, `frames` and `has_alpha`, each only when the file gives it. A file whose bytes do not decode under its kind's rule is refused. File facts enter an identity only through expressions that read them (`facts(x).width` in a `with:` value).

## 5. Text

**Decoding.** File content read as text is decoded as strict UTF-8; invalid UTF-8 is an error. One leading byte-order mark is removed. Line endings are kept as they are. The same rule applies to workflow inputs, cached outputs, prompt files and templates.

**Rendering.** When a value is interpolated into text (`"a ${{ x }} b"`, prompt files), `text(v)` is:

| Value | `text(v)` |
|---|---|
| string | itself |
| `true` / `false` | `true` / `false` |
| `null`, missing | the empty string |
| number | its JCS form: `1`, `0.30000000000000004`, `1e+21` |
| a text file | its decoded content |
| a JSON file | its content, compact, in the file's own key order, numbers in JCS form |
| any other file | `sha256:` and its digest |
| object or array | `canon(plain(v))` as a string |

A template that is exactly one `${{ … }}` and nothing else keeps the value's type instead of rendering it.

**Writing JSON.** When a node outputs a JSON value, the engine writes the file, so every SDK produces the same bytes and the same digest. The format is the canonical form spread over lines:
- keys in canonical order;
- one space of indentation per level;
- `": "` between a key and its value, `","` at the end of a line;
- strings and numbers exactly as in `canon`;
- empty objects and arrays as `{}` and `[]`;
- no trailing newline.

When every number is an integer written without `.0` (a Python `int`, not a whole `float`) or a decimal without an exponent, and every key lies in the Basic Multilingual Plane, this is byte for byte what Python's `json.dumps(v, sort_keys=True, indent=1, ensure_ascii=False)` writes.

**Prompt files** (a template param given as a project path, or `prompt.render`) are decoded as above. Then every HTML comment `<!-- … -->` is removed together with one newline directly after it, and the template is rendered. The rendered text, not the file's digest, is what enters the identity.

## 6. Node type identity

| Type | `type_identity` |
|---|---|
| built-in, `uses: fx/<name>@<major>` | `fx/<name>@<major>.<version>`, e.g. `fx/image.generate@1.1` |
| project type with a version, `uses: ./<path>#<attr>` | `<path>#<attr>@<version>`, where `<path>` is POSIX, relative to the project root, without `./` |
| project type without a version, `uses: ./<path>#<attr>` | `<path>#<attr>@source:` + `digest(source)`, with `<path>` as above |

For a project type without a version:

```
source = {
  "kind": "fx-node-source-v1",
  "files": { label: file_digest(file) for each file in the type's source closure },
  "resources": { path: file_digest(project_root / path) for each declared resource },
}
```

- The **source closure** is the module that defines the type plus every project module it imports, transitively. Which files that is depends on the language, so the node host reports it (see the node protocol's `describe`). The engine reads and hashes the files itself.
- A **label** is the file's path, POSIX, relative to the base it was found under: the project root, or a declared source package, whose files are labelled with the package's name followed by their path inside the package (`<package>/…`). When both bases hold the file, the nearer one counts.
- A **resource** is a project file the node reads at run time (a prompt, a schema). It is keyed by its declared path. A declared resource that does not exist inside the project is a refusal while planning.
- The **export**, `<path>#<attr>`, is part of the identity because a source does not name one type. Every type a module declares shares its source closure, types that declare the same resources share the whole source, and modules that import each other share one closure. Without the export, two unversioned types that receive the same values (§8) would have one step identity, and the second would be answered with the first's result.

Up to and including the 0.1.0 release, an unversioned type's identity was `source:` + `digest(source)`, without the export. Records made under that rule are not rewritten. An unversioned step planned now misses the result cache once and runs again; the paid calls it makes are still answered by the call cache, because a call key holds no type identity (§9). The plan digest of a workflow that uses an unversioned type changes with it (§10), so resuming a run folder planned under the old rule is refused: start a new run. Built-in and versioned types, `digest(source)` itself and `fx.lock` are unchanged.

A versioned type's source digest is not part of its identity. It is recorded in `fx.lock`, and planning refuses a type whose source no longer matches its lock entry: the author either bumps the version or confirms that behaviour did not change (`grida-fx lock --same`). A versioned type with no lock entry plans normally, and `grida-fx lock --check` reports it as unlocked.

## 7. Route fingerprint

A route is `<model>@<provider>`, split at the last `@`.

```
route_fingerprint = digest({
  "kind": "fx-route-v1",
  "capability": capability,
  "model": model,
  "provider": provider,
  "contract": contract,   // the route's contract object, completed as below; {} when none
})
```

Price, features, input-picture limits, concurrency and pacing are not part of it: repricing a route never invalidates a cache.

**Route tables.** The catalog is built from these tables, in this order:
1. the built-in default table, left out when any `--routes` file is given;
2. the tables listed in `fx.yaml` under `route_tables`, in their listed order;
3. each `--routes` file, in the order given.

A later table's entry for the same capability and route replaces an earlier one, contract included; this is how a project overrides a built-in price. Two entries for the same capability and route within one table are refused.

**The contract.** A route's contract is the object its entry declares, `{}` when none, completed when the table is read, in this order:

1. **`extends`.** An entry may name, in `extends`, another route of the same capability and provider, its base. The entry starts from the base's price, features, `max_input_pictures`, `concurrency`, `requests_per_minute` and contract. A member the entry states replaces the base's whole, except `contract`, whose members merge one level deep: a member the entry's contract states replaces the base's member of that name, and the base's other members stay. With a base, `price` may be left out.
   - The base is an entry already read, else the built-in default table's entry, which serves as a base even when a `--routes` file leaves that table out of the catalog. An entry already read is an earlier entry of the same table, else the entry in force in the catalog built so far, so it wins over a built-in entry of the same route. An entry may name its own route, to restate the entry it replaces.
   - The base is copied as it is when the entry is read: a later table that replaces the base changes no route that extended it.
   - `extends` itself is never hashed: an entry that extends a route and one that states the same members are the same route.
   - A base on another provider, or one that no table read so far holds for the entry's capability, is refused while the table is read.
2. **The serving adapter's identity.** Where one of FX's adapters serves the route's capability and provider ([providers.md](providers.md) §2), a contract that leaves out `adapter` gets that adapter's name, and one that leaves out `adapter_behavior` and whose `adapter` is that name gets that adapter's behaviour, typed exactly as that table writes it: the string `"1"` and the number `1` are different contracts. A member whose value is `null` counts as left out. A contract that names another `adapter` gets no behaviour. A route on a capability and provider no adapter of FX's serves keeps its contract as completed by step 1, even on a provider other capabilities of which FX serves (`vision.review` on `gg`).

Every route of the built-in table names both members, so step 2 changes none of their fingerprints.

A contract's `provider_options` ([adapters.md](adapters.md) §8) is a member like any other: it is hashed with the rest, and an entry that extends a route and states it replaces the base's whole. Up to and including the 0.6.1 release it meant nothing: GG's adapters and OpenRouter's chat adapters refused a contract that named it, and every other adapter ignored it. A route whose contract named it now sends its provider's member, or is refused ([adapters.md](adapters.md) §8), under the same fingerprint: no `adapter_behavior` moves, since the member had no meaning before and a new behaviour would move the fingerprint of every built-in route those adapters serve. A call cached before still replays, without the options.

Up to and including the 0.6.1 release, the contract was hashed as the entry declared it, and `extends` did not exist. Records made under that rule are not rewritten. A project route whose capability and provider one of FX's adapters serves, and whose contract left out `adapter` (or set it to `null`), or named that adapter and left out `adapter_behavior` (or set it to `null`), now has another fingerprint. Its steps miss the result cache once and run again, and its paid calls miss the call cache, since a call key holds the route fingerprint (§9), and are paid again. The plan digest holds the fingerprints too (§10), so resuming a run folder planned under the old rule is refused: start a new run. The kind stays `fx-route-v1`, the one exception to the rule at the top of this document: every built-in fingerprint is unchanged, and the routes that move are exactly the ones step 2 fills in.

## 8. Step identity

Each expanded step instance has an identity, the key of the result cache:

```
step_identity = digest({
  "kind": "fx-step-v1",
  "type": type_identity,
  "with": plain(with_values),
  "routes": { capability: route_fingerprint for each capability the type calls },
  "take": [take, …],
})
```

- `with_values` holds every input and param the instance receives, after evaluation:
  - each `with:` entry, evaluated;
  - every param left out of `with:` whose schema has a `default`, set to that default;
  - optional params with no default, and inputs not given, are absent.

  A `with:` name that is neither an input nor a param of the type is a refusal while planning.
- A string given to an input port that starts with `./` or `../` and holds no newline names a file, and becomes that file. In a workflow the path is relative to the workflow's home, the folder of the nearest `fx.yaml` above the workflow file; in an inputs file it is relative to that file. It must resolve inside the home. An absolute path, or one that leaves the home, is refused.
- `take` lists one number per regenerating level, outermost first: `[1]`, or `[2, 1]` for take 1 inside take 2 of a regenerating group.
- The step's path, name and instance id are not part of it. Identical work in two places has one identity and runs once in a run: an instance whose identity another instance of the same run is running waits for it, and the result cache then answers it. If that instance fails, the waiting one runs on its own. Runs in other processes that share the store do not wait for each other. Such work is priced once per phase as well: of a phase's live instances that share an identity, a plan's estimate and phase totals and a run's phase gate price only the first in listing order, and the gate prices nothing for an instance whose identity has a succeeded result in the run. A copy in another phase is priced again, since each phase is approved on its own price.
- If `with_values` contains a pending value, the identity is `null` until the values it waits on exist.

Upstream results enter by content: a file an upstream step wrote is `{"file": digest}`. An upstream step that runs again and writes the same bytes therefore leaves every downstream identity unchanged.

## 9. Call key

Each paid capability call is keyed by its request, the key of the call cache:

```
call_key = digest({
  "kind": "fx-call-v1",
  "capability": capability,
  "route": route_fingerprint,
  "request": plain(request),
  "take": [take, …],
})
```

The **canonical request** is the JSON the capability receives:
- input ports carry files as `{"file": digest}`;
- a param that names a text or JSON file carries the file's content (the decoded text, or the parsed JSON), because that content is what the provider receives;
- files a node writes and passes into a call are stored first and then referenced by digest;
- params the engine consumes itself are not part of it: a template's `vars` are spent rendering the prompt, and only the rendered prompt is sent.

A call record keeps its canonical request with nothing removed or added, because requests never carry secrets: keys travel in the transport, never in the request. A record is therefore enough to recompute its own key.

## 10. Plan digest

A run folder records the digest of the plan it ran. Resuming a run whose plan digest has changed is refused.

```
plan_digest = digest({
  "kind": "fx-plan-v1",
  "workflows": { source: document for the root workflow and every workflow it uses as a step },
  "inputs": plain(inputs),
  "takes": { step_path: take for each entry of the takes file },
  "types": { uses: type_identity for each distinct uses in the expanded plan },
  "routes": [ route_fingerprint for each capability and route the plan binds, sorted, distinct ],
})
```

- `document` is the workflow document as authored: the JSON value the strict YAML loader produced (or the document a builder returned), with no defaults filled in. Adding an optional field to the format therefore does not change existing plan digests.
- `source` is the document's project-relative POSIX path, or `<path>:<function>` for a builder.
- `routes` is a list because one route can serve several capabilities, each with its own fingerprint (§7).
- `inputs` are the values as given, after merging every inputs file and flag, before defaults are filled in. An optional input that was not given is left out.
- Whether a run answers its paid calls with a stand-in ([protocol.md](protocol.md) §5.7) is not part of the digest; `plan.json` records it apart ([store.md](store.md) §8).
- The canvas layout file ([layout.md](layout.md) §7, a ratified design not yet implemented) is not part of the digest: adding, editing or removing it changes no digest, so it needs no `kind` change.

## 11. Instance ids

An instance id is not a digest. It names an instance in records, logs and the takes file:

```
instance_id = step_path + "#" + takes joined with "."         e.g. draw#1, build.draw#2.1
```

A repeated step's path carries its key in brackets, quoted: `entity['ada'].draw#1`. Inside the quotes, `\` and `'` are escaped with a backslash. The key text of a repeat item is:
- a string, itself;
- a boolean, `true` or `false`;
- a number, its JCS form;
- a file, its name without suffix.

## 12. Money

- Prices and costs are US dollars with at most 6 decimal places.
- A route price or cap with more decimal places is refused when the route table is read.
- Engines compute money in whole micro-dollars (integers). Division rounds half to even.
- Money is written as JSON numbers. `0` and `0.0` are the same value (§1).
- A tiered price ([fx-routes-v1](schemas/fx-routes-v1.schema.json) `by` and `tiers`) prices a call at the tier its `by` setting names, when that setting is text. A setting that is absent, is not text, is not known to the plan yet, or names no tier prices the call at the route's whole range. A tier outside that range is refused when the route table is read.
- A capped route ([fx-routes-v1](schemas/fx-routes-v1.schema.json) `cap_usd`) has no known price: it prices each call at $0 to its cap, and each attempt holds the cap ([protocol.md](protocol.md) §6.1 step 6). A price that states no amount, neither `usd`, `low_usd`, `high_usd` nor `cap_usd`, is refused when the route table is read; it is not $0.

## 13. Content addressing, not a chain

Upstream step identities do not enter downstream identities. FX keeps content addressing (§8)
instead. Content addressing is already a Merkle structure over content. Unlike a chain of
identities, it lets an upstream rerun that writes the same bytes keep everything downstream cached.
A downstream identity that cannot be known until its upstream runs is inherent: the content does
not exist yet.

## 14. Worked examples

Each example gives the object, its canonical bytes and its digest. The same examples are in `vectors/identity/examples.json`, and `tools/digest.py --check-examples` reproduces every one. How a route's contract is completed (§7) cannot be seen in a hashed object, so `vectors/identity/routes.json` pins it from the tables a reader is given: a route `img-a@gg` serving `image.generate`, declared with no contract, has the contract `{"adapter":"gg-image","adapter_behavior":"1"}` and the fingerprint `7f6238d4aa013ab3e87587914c9ee915b141d33a2432a8ff1f3a361874901b95`.

**A file.** The 8 bytes `one\ntwo\n`:

    file_digest = c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8

**Numbers.** `1` and `1.0` are one value, `-0.0` is `0`, and the exponent form starts at 1e21:

    {"one": 1, "one_point_zero": 1.0, "tenth_sum": 0.1 + 0.2, "big": 1e21, "small": 1e-7, "e16": 1e16, "neg_zero": -0.0}
    canon  = {"big":1e+21,"e16":10000000000000000,"neg_zero":0,"one":1,"one_point_zero":1,"small":1e-7,"tenth_sum":0.30000000000000004}
    digest = 76f92b201d9f0e4475828136af59d52e04429713310ea236dd4311fa01138b49

**A route.** `img-a@acme` serving `image.generate`, with no contract:

    canon  = {"capability":"image.generate","contract":{},"kind":"fx-route-v1","model":"img-a","provider":"acme"}
    digest = 4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4

**A local step.** Conformance case `linear`, step `count`: the versioned project type `nodes/cases.py#lines@1` reads the file above.

    canon  = {"kind":"fx-step-v1","routes":{},"take":[1],"type":"nodes/cases.py#lines@1","with":{"text":{"file":"c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8"}}}
    digest = f8a9e8ecdbd14337e2bd5a3729b8992a2d6a91a51e41dc62841f5bb088c34578

**A built-in step.** Case `linear`, step `draw`, once `count` has found 2 lines. The param defaults `background` and `vars` are filled in:

    canon  = {"kind":"fx-step-v1","routes":{"image.generate":"4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4"},"take":[1],"type":"fx/image.generate@1.1","with":{"background":"auto","prompt":"A picture of 2 lines","vars":{}}}
    digest = 1beeb2e37052fc033ae6aabe0036b4932dbafea6b91d0b31be1b5e086064d142

**Its call.** `vars` is spent on rendering, so the request holds the prompt and the background:

    canon  = {"capability":"image.generate","kind":"fx-call-v1","request":{"background":"auto","prompt":"A picture of 2 lines"},"route":"4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4","take":[1]}
    digest = 9f66a9f2d23c7cf21331a3a45778bf9a660451a79cd4df3aae5f576e0b807d38

**A node's source.** The unversioned type `echo` in `nodes/n.py` (bytes `x = 1\n`), declaring the resource `prompts/r.md` (bytes `Hello\n`):

    canon  = {"files":{"nodes/n.py":"9e26bf369911c45c243c684147b23fc9e1dcfcf257d299a1c632016a6fcd33f4"},"kind":"fx-node-source-v1","resources":{"prompts/r.md":"66a045b452102c59d840ec097d59d9467e13a3f34f6494e539ffd32c1bb35f18"}}
    digest = f80b608fa143ba177212d039ea7c96232c844aefb446375b523c6108774327b3
    type_identity = nodes/n.py#echo@source:f80b608fa143ba177212d039ea7c96232c844aefb446375b523c6108774327b3

**A route with a contract.** `img-a@acme` serving `image.edit`, declared with a contract. The contract enters the fingerprint as written, arrays in their order:

    canon  = {"capability":"image.edit","contract":{"mask":true,"sizes":["1024x1024","1536x1024"]},"kind":"fx-route-v1","model":"img-a","provider":"acme"}
    digest = 5d727add4c417db9508eb6c7f81ff19f7d90005adf080c0702acb9077a330d6b

**A take two levels deep.** The built-in step above as take 1 inside take 2 of a regenerating group. Only `take` differs:

    canon  = {"kind":"fx-step-v1","routes":{"image.generate":"4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4"},"take":[2,1],"type":"fx/image.generate@1.1","with":{"background":"auto","prompt":"A picture of 2 lines","vars":{}}}
    digest = 081a272804f4af28b7e96bbc810966094959bc581b42fe10f402d8bbb7d64912

**Plain forms in a step's values.** An `fx/select@1` step whose `first_of` holds a failed upstream result, a missing value and a keyed collection of a file and a failed result (section 3):

    canon  = {"kind":"fx-step-v1","routes":{},"take":[1],"type":"fx/select@1.1","with":{"first_of":[{"failed":"draw#1"},{"missing":true},{"collection":[["ada",{"file":"c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8"}],["bo",{"failed":"entity['bo'].draw#1"}]]}]}}
    digest = 53c409bf47a7c2f5d831fd7987ae13bcc4d97647b468b51fbc1abe0686e13a65

**A text file with a byte-order mark.** The 11 bytes `EF BB BF` `Hello.\r\n`. The file digest covers every byte; read as text (section 5), the content is `Hello.\r\n`, with the mark removed and the line ending kept:

    file_digest = 280c3a0354d21e33d2877ed5a2234b37f951bc270ad66004e0731b961bde1c96

**A call with a text file param.** `voice-a@acme` serving `speech.generate`, and a call whose `text` param names that file. The request carries the decoded content, not the file's digest (section 9):

    canon  = {"capability":"speech.generate","contract":{},"kind":"fx-route-v1","model":"voice-a","provider":"acme"}
    digest = 3f38f8fe328b6466eb3ec1605239fab8e364c55ee340bd7e9ee49b2644993675

    canon  = {"capability":"speech.generate","kind":"fx-call-v1","request":{"text":"Hello.\r\n","voice":"narrator-a"},"route":"3f38f8fe328b6466eb3ec1605239fab8e364c55ee340bd7e9ee49b2644993675","take":[1]}
    digest = a789cf0e3723acd1caa7e89f4049e93fb445916b48c52676fdbdc26981c638d8

**A plan.** The workflow `workflows/tiny.yaml` draws with `fx/image.generate@1` and tints the drawing with `fx/image.edit@1`, both on `img-a@acme`; `img-a@acme` therefore serves two capabilities and enters `routes` twice, once per fingerprint (the routes above). The input `brief` is the file at the top, and the takes file sets `draw: { take: 2 }`. The workflow is hashed as authored, with no defaults filled in (section 10):

    canon  = {"inputs":{"brief":{"file":"c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8"}},"kind":"fx-plan-v1","routes":["4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4","5d727add4c417db9508eb6c7f81ff19f7d90005adf080c0702acb9077a330d6b"],"takes":{"draw":2},"types":{"fx/image.edit@1":"fx/image.edit@1.1","fx/image.generate@1":"fx/image.generate@1.1"},"workflows":{"workflows/tiny.yaml":{"fx":"workflow/v1","id":"tiny","inputs":{"brief":{"kind":"text/plain","type":"file"}},"steps":{"draw":{"route":"img-a@acme","uses":"fx/image.generate@1","with":{"prompt":"${{ inputs.brief }}"}},"tint":{"route":"img-a@acme","uses":"fx/image.edit@1","with":{"image":"${{ steps.draw.outputs.image }}","prompt":"Make it blue."}}},"title":"Tiny"}}}
    digest = 89d7ffdb27cd591501b572fcc8ea6260e794b5af27baa6a2aae0f9a86542a3c2

**Instance ids.** Names, not digests (section 11). The key `it's` of a repeated step, and the keys `a\b` and `1.5` at two levels of take 1 inside take 2:

    instance_id = entity['it\'s'].draw#1
    instance_id = set['a\\b'].shot['1.5'].draw#2.1
