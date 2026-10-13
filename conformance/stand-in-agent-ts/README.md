# stand-in-agent-ts

[`stand-in-agent`](../stand-in-agent/) in TypeScript: the node type `survey` in
`in/nodes/agent.ts` runs the engine's agent loop with one tool, `measure`, made with `tool()`,
and every paid call is answered by `answer` in `in/stand_in.ts` (`--stand-in
stand_in.ts#answer`), stand_in.py's answers written in TypeScript.

## The same call keys

An agent turn's request holds each tool's name, description and parameters, so they enter its
call key. Python derives `measure`'s from its docstring and signature; here they are written out
to the same values: the description `Measures a thing and says how tall it is.` and the
parameters `{"type": "object", "properties": {"thing": {"type": "string"}}, "required":
["thing"]}`. The `submit` schema is stand-in-agent's too. The engine computes every key, renders
the transcript, checks each answer and writes each JSON output, so with the same requests:

- `asked.json`, `answer.json`, `estimate.json` and `count.txt` are stand-in-agent's, byte for
  byte;
- `events-one.json` differs only where the node type is named: `uses` (`./nodes/agent.ts#survey`),
  the plan digest, and the step identities of `survey#1` and `garbled#1`, which hold the type
  identity `nodes/agent.ts#survey@1`.

## What the case pins

- The TypeScript host serves the engine's `tool.invoke` for the body's tool while `agent.run` is
  pending; the tool records what it measured in `ctx.state`, which the body reports as a node
  fact.
- An `agent.turn` answer the engine refuses (`garbled`) and a structured answer its schema
  refuses (`wrong`) are `call_failed`, which `survey`'s body lets through, as Python's does.
