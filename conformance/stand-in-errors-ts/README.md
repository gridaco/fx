# stand-in-errors-ts

[`stand-in-errors`](../stand-in-errors/) with its stand-in written in TypeScript, `answer` in
`in/stand_in.ts`, and its one node type, `asks_nothing` in `in/nodes/asks.ts`. The stand-in
answers each prompt as `stand_in.py` does; what differs is only how an answer is written in
TypeScript ([protocol.md](../../spec/protocol.md) §8.2):

| Prompt | `stand_in.py` | `stand_in.ts` |
|---|---|---|
| `refuse this` | `raise CallRefused(…)` | `throw new CallRefused(…)` |
| `fail this` | `raise CallFailed(…)` | `throw new CallFailed(…)` |
| `decline this` | `return DECLINE` | `return DECLINE` |
| `text instead` | `Output(kind="text/plain", data=…)` | `{bytes, kind: "text/plain"}` |
| `no file at all` | `Answer()` | `{}` |
| `data as well` | `Answer(files=…, data={"seed": 7})` | `{files, data: {seed: 7}}` |

Its pictures come from `in/lib/png.ts`, which writes the same bytes as `stand_in.py`'s `png()`
([stand-in-calls-ts](../stand-in-calls-ts/README.md)).

The engine checks every answer and words every failure, so the case pins the same sentences and
error codes as stand-in-errors, and `asked.json`, `count-one.txt` and `count-two.txt` are
stand-in-errors', byte for byte. The records differ only where they name the node type
`asks_nothing`: `uses`, the plan digest, and `body#1`'s step identity.
