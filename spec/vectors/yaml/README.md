# YAML vectors

Each vector pins a rule of [yaml.md](../../yaml.md).

- **`accept/<name>.yaml`** must load, and the value must equal the one in `accept/<name>.json`. Compare values, not text: canonicalize both ([identity.md](../../identity.md) §2) and compare the bytes. The JSON files are pretty-printed, with sorted keys and numbers in their JCS form, so `1.0` is written `1`.
- **`refuse/<name>.yaml`** must be refused. `refuse/<name>.txt` is one line naming the rule in yaml.md that refuses it; an implementation does not have to print that text. Apart from the encoding vectors (`stream_invalid_utf8`, `stream_utf16`, `stream_bom_twice`), every refused file is well-formed YAML 1.2, so it is refused by the rule and not by a syntax error.

The `.yaml` files are bytes and must be read without any conversion. `stream_bom.yaml` and `stream_bom_only.yaml` start with a byte-order mark and `stream_bom_twice.yaml` with two, `stream_empty.yaml` is zero bytes, `stream_invalid_utf8.yaml` and `stream_utf16.yaml` are not UTF-8, and the `line_break_*.yaml` files hold U+0085, U+2028 or U+2029, which some editors turn into line breaks.

An accept vector's value need not be a mapping (`top_level_*`): the subset reads any top-level value, and only the schemas of FX's documents require a mapping.
