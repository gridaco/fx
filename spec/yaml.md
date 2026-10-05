# The YAML subset

FX reads YAML for project files, workflow files, route tables, inputs files, takes files and `fx.lock`. Every YAML library resolves plain scalars a little differently, and YAML 1.1 turns `on` into `true` and `017` into 15. FX therefore reads a strict subset of [YAML 1.2](https://yaml.org/spec/1.2.2/), which gives each document exactly one meaning. Anything outside the subset is refused with its line and column, never guessed at.

The vectors in `vectors/yaml/` pin every rule: `accept/*.yaml` with the JSON each one means, and `refuse/*.yaml` with the reason each one is refused.

## Streams

- **Encoding.** A stream is UTF-8. One leading byte-order mark is allowed and ignored; a second is refused.
- **Line breaks.** U+0085, U+2028 and U+2029 are refused anywhere in a stream. YAML 1.1 treats them as line breaks and YAML 1.2 as content, so they have no single meaning.
- **One document.** A stream holds one document. An explicit `---` start marker and a final `...` end marker are allowed; a second document is refused.
- **Empty document.** An empty stream, one holding only comments, or an explicit `---` with no content is the empty mapping `{}`.
- **Top level.** The top-level value may be any value. The documents FX defines are mappings, and their schemas refuse anything else.

## Not supported

These are refused:
- anchors and aliases (`&name`, `*name`);
- merge keys (`<<`);
- tags (`!!str`, `!custom`);
- directives (`%YAML`, `%TAG`, and reserved ones such as `%FOO`);
- complex keys (`? `);
- a key that is itself a collection.

## Mapping keys

- **Keys are strings.** A plain key is taken as its text: `on:`, `1:` and `true:` are the keys `"on"`, `"1"` and `"true"`. A quoted key is its unquoted text; a quoted `"<<"` is an ordinary key.
- **No duplicates.** Two equal keys in one mapping are refused.

## Values

Quoted scalars (single or double) and block scalars (`|`, `>` and their chomping indicators) are always strings.

A plain scalar used as a value resolves as follows, in this order:

| Plain scalar | Value |
|---|---|
| `null`, `~`, or nothing (`key:`) | null |
| `true`, `false` | boolean |
| decimal integer: `0`, `-0`, `+0`, or an optional sign and digits not starting with `0` | number (`-0` and `+0` are 0). Refused if reading it would round it ([identity.md](identity.md) §1). |
| decimal float: `[-+]?(\.[0-9]+\|[0-9]+\.[0-9]*\|[0-9]+)([eE][-+]?[0-9]+)?` whose integer part has no leading zero | number. Refused if it overflows to infinity. |
| an **ambiguous** form: see below | refused, with "quote it" |
| anything else | string |

The ambiguous forms are plain scalars that some YAML version or library reads as a non-string:
- `yes`, `no`, `on`, `off` in any letter case;
- `true`, `false`, `null` in any letter case other than all lower case (`True`, `FALSE`, `nULL`);
- numbers with a leading zero: `017`, `01.5`, `00.5`, `01e3`;
- `0x` followed by hex digits, `0o` by octal digits, `0b` by binary digits (underscores included), in either case of the prefix letter, with an optional sign;
- anything that would be a number if its underscores were removed: `1_000`, `.5_0`;
- sexagesimal numbers: digits separated by colons, with an optional sign and an optional fractional part: `1:30`, `16:9`, `-1:30`, `1:30.5`;
- `.inf` with an optional sign, and `.nan` without one, in any letter case;
- dates and timestamps, as YAML 1.1's timestamp type writes them: `YYYY-MM-DD` (`2026-10-05`), or a date with one- or two-digit month and day followed by `T`, `t` or spaces and a time (`2026-10-05T10:00:00Z`, `2026-1-5 9:30:00`).

Words that only look similar are strings: `0bad`, `0ops`, `-.nan`, `2026-1-5`.

A refused value is fixed by quoting it: `"on"`, `"017"`, `"2026-10-05"`.

`y` and `n` are strings: YAML 1.1 lists them as booleans, but common loaders (PyYAML among them) never resolved them that way, and words like `x` and `y` are ordinary values in workflows.

## Numbers

A number is one value regardless of how it is written ([identity.md](identity.md) §1): `1`, `1.0` and `1e0` mean the same thing.

## Writing YAML

When FX writes YAML (`fx.lock`, takes files), it quotes every string that would otherwise resolve to something else or be refused. A digest such as `123e4567…` is therefore always written in quotes.
