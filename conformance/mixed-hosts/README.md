# mixed-hosts

One workflow of Python and TypeScript node types ([protocol.md](../../spec/protocol.md) §1): the
engine picks each module's host by its suffix, `nodes/words.py` the Python host and
`nodes/words.ts` the TypeScript one, and keeps one planning host and one pool of node hosts per
language. The case needs both: `GRIDA_FX_PYTHON` and the built `@grida/fx` on Node 24.

## The data, both ways

| Step | Language | Reads | Makes |
|---|---|---|---|
| `order` | TypeScript | | a keyed collection, a `Map` in the order `sun`, `10`, `2` |
| `keys` | Python | `order`'s collection, as a `dict` | text: each key and its file's text, in the order it gets them |
| `framed` | TypeScript | `keys`' text | the text in brackets (`framed.txt`) |
| `report` | TypeScript | | JSON: the words of a phrase and their letters |
| `measure` | Python | `report`'s JSON | a node fact, `letters`, and a list of one text per word (`sizes-0.txt` …) |
| `tally` | TypeScript | `measure`'s list | the list's texts, in order, and a node fact, `lines` (`tally.txt`) |

Collections travel as `[key, ref]` pairs, so their order survives both hosts: `framed.txt` lists
`sun`, `10` and `2` in that order.

## What the case pins

- The run succeeds, and `project.json` shows each instance with its node facts.
- `lock` locks the six versioned types of both languages in one `fx.lock`; each module's types
  share their module's source digest.
