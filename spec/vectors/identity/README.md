# Identity vectors

Each file pins a part of [identity.md](../../identity.md). `tools/digest.py --check-examples` checks all three.

- **`examples.json`** (`"kind": "fx-identity-examples-v1"`): the worked examples of identity.md §14.
  - An example with `object` (or `object_source`, a JSON text) must canonicalize to `canonical` and hash to `digest`. A hashed object names its formula in `kind`, and rebuilding it through that formula must give the same digest.
  - An example with `file_utf8` is a file of those UTF-8 bytes; `digest` is its file digest, and `text`, when present, is its content read as text (§5).
  - An example with `instance_id` is a name, not a digest (§11): `path` lists each level as `{"step"}`, with `"key"` on a repeated step, and `take` is the instance's take.
- **`markers.json`** (`"kind": "fx-marker-vectors-v1"`): reserved plain markers (§3). Each case's `json` is a JSON text. Read as an authored value (an inputs file, a workflow, a table, a JSON file), it must be refused when `refuse` is true and read as ordinary data when it is false. Every case is a valid I-JSON value, so a reader of runtime values (a graph, a record) reads all of them.
- **`json_output.json`** (`"kind": "fx-json-output-vectors-v1"`): the bytes a JSON output is written as (§5, "Writing JSON"). Writing the value of `value_json` must give exactly the UTF-8 bytes of `bytes_utf8`.
