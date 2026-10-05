# The node protocol

`fx-node-protocol-v1` · schema: [schemas/fx-node-protocol-v1.schema.json](schemas/fx-node-protocol-v1.schema.json)

The engine runs node bodies and workflow builders written in other languages through a **node host**: a separate process that loads the user's modules and runs their code. The Python host ships with `grida.fx`; a TypeScript host comes later. This document is the contract between the engine and any host. (The [overview](../docs/wg/overview.md) calls it "protocol v2": its design name relative to the Python engine FX grew out of, which ran bodies in its own process.)

The split is fixed. **The host runs user code and nothing else.** The engine owns the store, the call cache, routes, prices, budgets, retries, prompt rendering, the agent loop, file facts and every identity. A host never computes a digest or a file fact, never talks to a provider and never writes the store. The values a body reports about its own result with `fact` are *node facts* (§6.3), not file facts ([identity.md](identity.md) §4).

The words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

## 1. Transport

**Starting a host.** The engine starts the host with its working directory at the project root and talks over the host's stdin and stdout.
- A project module is hosted by the host of its language, chosen by suffix: `.py` is Python. The built-in types whose bodies live in `grida.fx.std` are hosted by the Python host.
- The Python host is `<python> -P -m grida.fx.host`. `<python>` is `GRIDA_FX_PYTHON` when it is set; otherwise the project's `.venv` interpreter (`.venv/bin/python`, or `.venv\Scripts\python.exe` on Windows) when it exists; otherwise `python3` on `PATH`.
- `-P`, Python's safe-path option (Python 3.11 and later), keeps the working directory off `sys.path`. The project root is therefore not on `sys.path` until `initialize` puts it there, after the host has imported what it needs, and a project module named like a standard library module (`json.py`, `typing.py`) or like `grida` cannot replace the host's own imports. The engine also sets `PYTHONSAFEPATH=grida-fx` in the host's environment when the variable is unset or empty, for an interpreter wrapper that drops `-P`. The host removes a `PYTHONSAFEPATH` of exactly that value before it loads user code, so the programs user code starts inherit the user's environment.

**Framing.** Each message is a header, `Content-Length: <n>\r\n`, an empty line `\r\n`, and then n bytes of UTF-8 JSON, as in LSP's base protocol. n counts bytes, not characters. A reader MUST ignore other header fields; a sender SHOULD write none.

**Messages.** Messages are JSON-RPC 2.0 requests, notifications and responses. Batches are not used. Every message MUST lie in the I-JSON domain ([identity.md](identity.md) §1): a sender never writes `NaN` or an infinity, and a receiver refuses a message that holds one with `-32700`.

**Both directions.** Both sides send requests and notifications.
- An id is an integer or a string, unique per sender for the session. A sender SHOULD count up from 1.
- The two sides' ids are independent. A message with `method` is a request (or notification) from its sender; a message without one answers a request the receiver sent.

**Interleaving.** Requests may be outstanding in both directions at once. While a `run` the engine sent is pending, the host sends `capability`, `fact` and the rest, and the engine sends `tool.invoke` and `agent.check` for the same run. Each side MUST keep reading and answering while its own requests are pending. Responses may arrive in any order.

**One job per host.** A host handles one `describe`, `build` or `run` at a time: the engine sends the next only after the previous one is answered. To run nodes in parallel, the engine starts more hosts and reuses them. A builder changes the working directory, bodies share module state, and a run that has to be killed should take nothing else with it.

**Output streams.** stdout carries protocol messages only. A host MUST keep whatever user code prints off it: the Python host duplicates file descriptor 1 for the protocol and points descriptor 1 at stderr before it imports any user code. stderr is free-form log text. The engine may show or keep it and never parses it.

**End of input.** A host MUST exit when its stdin reaches end of file. Neither end of input nor `exit` waits for threads that user code started: the Python host flushes and closes the protocol stream, then ends the process at once (`os._exit`).

## 2. Session

```
engine → host   initialize {protocol, engine, project_root, sources}  →  {protocol, host}
                describe / build / run, with the host's requests inside each run
engine → host   shutdown  →  null
engine → host   exit (notification)
```

**`initialize`** MUST be the engine's first message. Until it is answered, the host answers any other request with `-32600`.

| Param | |
|---|---|
| `protocol` | `"fx-node-protocol-v1"` |
| `engine` | `{name, version}`, e.g. `{"name": "grida-fx", "version": "0.1.0"}` |
| `project_root` | the absolute path of the folder holding `fx.yaml` |
| `sources` | the project's declared source packages, `[]` when there are none |

The result is `{protocol, host: {language, version, sdk_version}}`. `version` is the language runtime's and `sdk_version` the SDK's (for Python, the `grida` distribution's).

**Version mismatch.** A host that does not speak the engine's `protocol` answers with `protocol_mismatch`. Its message names both versions, and its `data` is `{engine_protocol, host_protocol}`. An engine that gets back a `protocol` other than its own fails the session the same way. Either way the engine reports both versions and what to upgrade, for example `the project's grida 0.1.0a1 speaks fx-node-protocol-v1 and grida-fx 0.2.0 speaks fx-node-protocol-v2: upgrade grida`, and then ends the host.

**`shutdown`** takes no params. The host stops accepting work and answers `null`. The engine then sends the **`exit`** notification, and the host exits with status 0, or with 1 if no `shutdown` came first. The engine MAY kill a host that has not exited 5 seconds after `exit`.

A host loads each module at most once per session. The engine starts a new session for each command it runs, so a session never outlives an edit it should have seen.

## 3. Shared values

### 3.1 File refs

Every file the engine hands a host is a **file ref**:

| Field | |
|---|---|
| `digest` | `file_digest` ([identity.md](identity.md) §2) |
| `kind` | the file kind ([identity.md](identity.md) §4) |
| `size` | bytes |
| `name` | a display name. It is never part of an identity. |
| `key` | the item's key in a keyed collection; absent or `null` otherwise |
| `path` | the absolute path of the file in the store. The host reads it and MUST NOT write, move or delete it. |
| `facts` | the file facts `facts(f)` ([identity.md](identity.md) §4), computed by the engine |

The engine MUST include `facts` in every file ref it sends. A file ref is valid only within the run that received it.

### 3.2 Files inside JSON values

When a host puts a file inside a JSON value (a capability request, prompt variables, agent pictures, tool pictures), it writes `{"file": "<digest>"}`. This is the file's plain projection ([identity.md](identity.md) §3), so a capability request on the wire already is its canonical request.

- The digest MUST name a file the engine handed this run: an input, a param file, a file from a capability result, or a `file.put` result. Any other digest is refused with `unknown_file`.
- A file the body made itself is stored with `file.put` first.
- An object of exactly that shape is always read as a file.

**Reserved markers.** JSON values a host sends, in a request or in a `run` result, follow the reserved-marker rule of [identity.md](identity.md) §3, since they can enter an identity, a record or a stored file. That covers node facts, capability requests, prompt variables, marks, `file.put` JSON values, and an agent's tool parameters and `submit` schema. A value that holds a reserved marker object, in a shape listed there, is refused with `-32602`: a request is answered with that error, and a `run` result fails the node with it (§5.3). The one exception is a file value where this section puts one: in a capability request, in prompt variables, and as agent and tool pictures. A tool's `content` (§5.4) is not checked, because it reaches the transcript only as text.

### 3.3 Staged inputs

In `run`, each input port's value depends on the port's shape:

| Shape | Value |
|---|---|
| one file | a file ref |
| list, `kind[]` | `{"list": [file ref, …]}` |
| keyed, `kind{}` | `{"collection": [[key, file ref], …]}`, in collection order |

An optional input that does not exist is absent.

### 3.4 Output values

A body returns each output port as:
- `{"work_path": "<path>", "kind": "<kind>"}`: a file the body wrote. `work_path` is POSIX, relative to the run's `work_dir`, and MUST stay inside it. `kind` defaults to the suffix rule ([identity.md](identity.md) §4).
- `{"file": <file ref>}`: a file the engine handed this run (an input, a capability result's file, a `file.put` result), passed through by digest.
- For a list port, `{"list": [value, …]}`; for a keyed port, `{"collection": [[key, value], …]}`.
- `null`, or no entry: no output. Only an optional port may be left out.

The engine records the kind it is given and checks the shape, not the kind, against the port.

## 4. Node types as data

`describe` returns each node type as a **type spec**:

| Field | Type | |
|---|---|---|
| `name` | string | words of `[a-z][a-z0-9_]*` joined by `.` |
| `description` | string, optional | the body's documentation (Python: its docstring), for `grida-fx nodes`. Never part of an identity. |
| `inputs` | `{name: port}` | the files it reads |
| `params` | `{name: schema}` | the JSON values it is set with |
| `outputs` | `{name: port}` | the files it returns |
| `judge` | boolean | a judge reports the node fact `verdict`, `accept` or `reject` |
| `calls` | `{capability: number or param name}` | the most paid calls of each capability in one run |
| `resources` | `[path]` | project files the body reads at run time (prompt files, schemas): POSIX, relative to the project root, no `..` |
| `tools` | `[string]` | external programs: a name matching `^[a-z0-9][a-z0-9_-]*$`, optionally followed by a version bound that starts with one of `<>=!~`, as in `blender>=4.2` |
| `view` | string or null | a view template, kept for the views work (not in this version) |
| `version` | integer ≥ 0, or null | null for an unversioned type, whose identity is its source ([identity.md](identity.md) §6) |
| `retry` | `"service"` or `"engine"` | see below |

Input, param and output names match `^[a-z][a-z0-9_]*$`. No name is both an input and a param.

**Ports.**

```
port    = kind [ "[]" | "{}" ] [ "?" ]
kind    = family [ "/" subtype ]
family  = image | audio | video | model | text | json | annotations | file
subtype = 1*( a-z / 0-9 / "." / "+" / "-" )
```

`image` is one file, `image[]` a list, `image{}` a keyed collection, and a trailing `?` makes the port optional. A kind is a family (`image`) or a media type in it (`image/png`).

**Params.** Each param is a JSON Schema (draft 2020-12) object. FX's own keywords are the extensions `x-fx-file`, `x-fx-template` and `x-fx-optional`; the last two apply to a param (`x-fx-file` tags file paths in the input schemas `grida-fx schema` writes). The engine also acts on the standard `default`:
- `x-fx-template: true` marks a string that the engine renders before the run: a template, or a project path to a prompt file ([identity.md](identity.md) §5). The body receives the rendered text.
- `x-fx-optional: true` lets a step leave the param out with no default. The body then does not receive it.
- `default` (standard JSON Schema) is filled in when the step leaves the param out ([identity.md](identity.md) §8).

A param with neither `default` nor `x-fx-optional` is required, and a step that does not set it is refused while planning. SDK shorthand is expanded by the SDK: Python's `int` becomes `{"type": "integer"}`, and a tuple of choices becomes `{"enum": […]}`.

**Calls.** A bound is a number of at least 1, or the name of an integer param whose schema has a `minimum` of at least 1 and a `maximum`. A named bound is the step's value of that param, or the param's `maximum` while that value is not known. The engine prices the plan from these bounds and refuses a call past them (`over_bound`).

**Retry.**
- `"service"` (the default): a body runs once. Its paid calls are retried by the engine's call retry, never by running the body again.
- `"engine"`: after a `node_error`, the engine runs the body again in a fresh work directory, at most 6 runs in all. Calls an earlier attempt completed are answered from the call cache at no cost.

**Validation.** The engine validates every spec it receives against these rules and refuses an invalid one while planning. An SDK SHOULD refuse it when it is declared.

**Engine types.** The body-less built-ins belong to the engine: the paid capability types (`fx/image.generate@1`, …) and `fx/select@1`. A host never describes them, and the engine never sends `run` for them.

## 5. Engine to host

### 5.1 `describe`

Loads modules and reports their node types and source closures.

| Param | |
|---|---|
| `targets` | `[{path, attribute?}]`. `path` is a project module, POSIX, relative to the project root (`nodes/cases.py`). With `attribute`, only that module-level name is described; without it, every module-level attribute that holds a node type. |
| `builtins` | true to also report the built-in types this host carries |

The result is `{modules, builtins}`:
- `modules` has one entry per target, in order: `{path, types: [{attribute, spec}], closure}`, or `{path, attribute?, error}` when the module fails to load, the attribute is missing, or it is not a node type. Errors read like `nodes/x.py failed to import: ModuleNotFoundError: No module named 'foo'`.
- `builtins` is `[{uses, spec}]`, such as `{"uses": "fx/image.resize@1", …}`. Every built-in spec has a `version`; its identity is `fx/<name>@<major>.<version>`.

**Source closure.** `closure` lists the module and every project module it imports, transitively ([identity.md](identity.md) §6). Which files those are depends on the language's import rules, so the host computes the list; the engine reads and hashes the files itself, and computes the type identity and the `fx.lock` check. For Python:
- every `import a.b` in the file's syntax tree names `a.b`, and every `from m import x` names both `m` and `m.x`. Relative imports resolve against the importing file's package. Imports count wherever they appear (inside functions and conditionals too), whether or not they run.
- A name is in the closure when it resolves to `<base>/a/b.py` or `<base>/a/b/__init__.py`. The base is the project root, or, for a name whose first part is a declared source package, that package's parent folder.
- A file counts only when every part of its path below the base is spelled exactly as its directory entry, case included, as Python's own import requires. The closure is then the same on file systems that ignore case and on those that do not: `from Lib import Helper` names nothing when the file is `lib/helper.py`.

Each entry is `{label, path}`. `label` is the file's path relative to the nearer of the project root and its source package's parent, POSIX. `path` is the absolute path the engine reads; it never enters a record.

Loading a module runs its top-level code, with imports resolved against the project root first. A module outside the project root is an error. Whatever the module raises while it loads is its error, `SystemExit`, `KeyboardInterrupt` and `asyncio.CancelledError` included; it never ends the host.

### 5.2 `build`

Runs a workflow builder (`grida-fx run workflows/levels.py:build`).

| Param | |
|---|---|
| `path` | the builder file, relative to the project root |
| `function` | the builder function's name |
| `arguments` | `{name: string}`, given to the function as keyword arguments |
| `cwd` | the engine's working directory, absolute. The host loads the builder module (running its top-level code) and runs the builder there, since a builder reads its own files relative to where `grida-fx` runs, then returns to its own working directory. |

The result is `{document, takes_anchor}`:
- `document` is the workflow as `fx: workflow/v1`, exactly as the SDK's `Workflow` writes it, with no defaults filled in. The engine validates it like a workflow file. It enters the plan digest under `<path>:<function>` ([identity.md](identity.md) §10).
- `takes_anchor` is the project-relative path of the module that constructed the `Workflow` (in Python, the file that called `Workflow(…)`), or `path` when that module is unknown or outside the project. The engine keeps the workflow's takes file, `<workflow id>.takes.yaml`, in the anchor's folder.

Errors: `load_failed` (no such file or function, or an import failed) and `build_failed` (the builder raised anything, `KeyboardInterrupt` and `asyncio.CancelledError` included, or returned something other than a `Workflow`). A `build_failed` message reads `<function>: <exception type>: <message>`, or `<function>: <exception type>` when the message is empty.

### 5.3 `run`

Runs one instance's body. The engine sends `run` only for an instance that has to run: a result-cache hit, `fx/select@1` and the body-less built-ins never reach a host.

| Param | |
|---|---|
| `run_id` | a string unique in the session. Every host request made for this run carries it. |
| `instance` | `{id, path, step, key, take}`: the instance id ([identity.md](identity.md) §11), its step path with repeat keys, its declared step path, its repeat key or `null`, and its take: a list with one take number per regenerating level, outermost first, as in the step identity ([identity.md](identity.md) §8) |
| `type` | the type identity ([identity.md](identity.md) §6) |
| `body` | `{path, attribute}` for a project type, or `{builtin: "fx/<name>@<major>"}` for a built-in this host carries |
| `params` | the instance's param values. Templates are already rendered and defaults filled in. A text or JSON file given to a param arrives as its content: decoded text, or parsed JSON ([identity.md](identity.md) §5). A missing value arrives as `null`. A param the step left out and that has no default is absent. |
| `param_files` | `{pointer: file ref}`: every other file inside `params`, keyed by its RFC 6901 JSON pointer into `params`. The value at that pointer is `null`. |
| `inputs` | `{port: staged input}` (§3.3) |
| `work_dir` | the absolute path of an empty directory the engine made for this run. The body may write anything under it. The engine deletes it after storing the outputs. |
| `resources` | `{declared path: absolute path}` for each declared resource |
| `tools` | `{name: executable or null}` for each declared tool, keyed by its name without the version bound. The engine resolves `GRIDA_FX_TOOL_<NAME>`, where `<NAME>` is the name in upper case with each `-` written as `_` (`GRIDA_FX_TOOL_BLENDER`), when it is set, else `PATH`. `null` means not found; using it fails the node. |
| `calls` | `{capability: bound}`, resolved for this step. Informative: the engine enforces it. |
| `timeout_s` | the step's timeout in seconds, or `null` |

The result is `{outputs, facts?, marks?}`: `outputs` maps each output port to an output value (§3.4), and `facts` maps names to node facts. A host MUST NOT answer `run` while a request it sent for that run is pending. If it does anyway, whatever the engine is doing for that run, such as a paid call already sent, still completes and settles.

After the host answers, the engine:
1. merges the node facts reported with `fact`, in order, with the result's `facts` on top: a later value for a name replaces an earlier one. Marks are those reported with `annotate`, followed by the result's `marks`.
2. checks the result: no undeclared output port, every non-optional port present, each value's shape matching its port; no fact named `cost_usd`, which is the engine's (step 5), and no reserved marker in a fact or a mark (§3.2). A failed check fails the node, with `-32602` for a refused fact or mark, and it is not retried.
3. writes `{"kind": "fx-annotations-v1", "annotations": marks}` as the `annotations` output when the type declares one, the body returned none, and there are marks.
4. requires a judge's `verdict` node fact to be `accept` or `reject`.
5. stores the output files, sets the node fact `cost_usd`, and writes the result record (`fx-result-record-v1`) under the step identity when that is known. The engine always writes `cost_usd` itself: what the run's paid calls cost, or `null` when it made none, the same value as the result record's `cost_usd` ([store.md](store.md) §3).

**Failure.** The host answers `run` with an error:
- `node_failure` when the body failed on purpose (Python: `raise ctx.fail(…)` or `NodeFailure`);
- `node_error` when it raised anything else, with the exception's type and traceback in `data`;
- the error the engine answered one of the run's requests with, when the body let it propagate, with its code and message unchanged (`ceiling_exceeded`, `call_failed`, …).

For `node_failure` and `node_error`, `data` MAY carry node `facts` and `marks` the body kept locally, under the same rules as a result's. The engine merges them as above, leaving out any that step 2 would refuse, so a failed node keeps its node facts and its own error.

**Timeouts and cancellation.** At `timeout_s`, or when a person stops the run, the engine sends `$/cancel` for the `run`. A host that has not answered 5 seconds later is ended, together with what it started where the platform allows that. A node that timed out fails with `ran past <n> seconds` and is not retried. A host that exits while a run is pending fails that run as a `node_error` would, with `the node host exited with status <n>`.

### 5.4 `tool.invoke`

The engine calls one of the body's agent tools while the body's `agent.run` is pending.

| Param | |
|---|---|
| `run_id`, `agent_id` | the run, and the agent the body opened |
| `call_id` | the model's id for the tool call, or `null` |
| `name`, `arguments` | the tool and the JSON object the model filled in |

The host calls the tool: in Python, `function(ctx, **arguments)` for an `@tool` function, or `tool.handler(arguments)` for a declared tool. The result is one of:
- `{"content": <JSON value>, "images": [file value, …]}`: the tool's answer. `images` is optional and holds pictures the host stored with `file.put` or received as file refs.
- `{"error": "<exception type>: <message>"}`: the tool raised. The model is told the text, and the loop goes on.

A tool that fails the node answers with a `node_failure` error. That ends the agent loop, and the engine answers `agent.run` with the same error.

### 5.5 `agent.check`

`{run_id, agent_id, value}`: the engine asks the body's check function about a submitted value that has already met the `submit` schema. The result `{"refusal": null}` accepts it. `{"refusal": "<why>"}` refuses it; in Python the check raised `ValueError` or `NodeFailure`. Any other exception is answered with `node_error`, which ends the agent loop like a tool's `node_failure`.

### 5.6 `$/cancel`

A notification, `{id}`, naming a pending request the engine sent. The host SHOULD stop that work and answer the request with `cancelled`. For a `run`, the Python SDK sets `ctx.cancelled`, which a body checks. The engine discards a result that arrives after it cancelled the request.

## 6. Host to engine

Every request in this section carries the `run_id` of a pending `run`. A request with any other `run_id` is refused with `-32602`, and such a notification is ignored. When the engine stops a run, it answers that run's pending host requests with `cancelled`.

### 6.1 `capability`

A paid call: `{run_id, capability, request}`. `request` is the canonical request ([identity.md](identity.md) §9): JSON, with each file as a file value (§3.2). The engine:
1. checks that the type declares the capability (`capability_undeclared`) and has calls left (`over_bound`). Cache hits count toward the bound.
2. takes the instance's route for the capability (`no_route`, which also covers a route no adapter serves).
3. computes the call key ([identity.md](identity.md) §9) and answers from the call cache when it can. This comes before the live check, so a dry run replays the calls it has already paid for.
4. refuses with `not_live` when the run is not live: a dry run, or an `at: plan` step.
5. looks up the call's job record ([store.md](store.md) §5):
   - `submitting`: an earlier submission has an unknown outcome. The engine refuses with `job_unsettled`, and the message says to check the provider's dashboard and then run `grida-fx jobs --forget <key>`.
   - `submitted`: the engine collects the job by its `handle`, with no new hold and no new submit, because the run that submitted it was charged its hold. An answered job goes on at step 8. A job that ends without a result leaves its record `settled` and fails the call with `call_failed`; a later run submits it anew.
   - no record, or a `settled` one: the call goes on at step 6, as a new submission.
6. reserves the route's high price under the run's ceiling and any step budget (`ceiling_exceeded`, with `needed_usd` and `remaining_usd` in `data`).
7. makes the call as its only retry owner: at most 6 attempts, each reserved, recorded and settled. `capability_refused` means the adapter refused before anything was sent, settled at $0. `call_failed` means every attempt failed. A long job keeps its job record as [store.md](store.md) §5 describes.
8. stores the files and the call record (`fx-call-record-v1`), and then removes the call's job record, if any.

The result is `{key, cached, cost_usd, files: {name: file ref}, data}`. `cost_usd` is 0 on a hit and for a collected job, which bills nothing new, and `null` when the provider reported no cost (the engine then charged the whole hold).

### 6.2 `agent.run`

The engine runs a whole tool-using model loop for the body.

| Param | |
|---|---|
| `agent_id` | chosen by the host, unique in the run. `tool.invoke` and `agent.check` name it. |
| `system`, `instructions` | the system prompt, and the opening user message |
| `images` | optional: pictures for the opening message, as file values |
| `tools` | `[{name, description, parameters}]`: the body's tools, each served through `tool.invoke`. `parameters` is a JSON Schema. The name `submit` is reserved. |
| `max_steps` | the most turns, at least 1 |
| `submit` | a JSON Schema the final answer must meet, or `null` for a text answer |
| `check` | true when the body has a check function: every submitted value that meets `submit` then goes to `agent.check` |
| `recent_images`, `max_tokens` | optional; see below |

The loop, which fixes every turn's request and therefore its call key:
1. The transcript starts with `{"role": "user", "content": instructions}`, plus `"images"` when there are any.
2. The tools sent are the body's, in order. With `submit`, they are followed by `{"name": "submit", "description": "Finish: submit the answer this task asks for.", "parameters": submit}`.
3. Each turn is one `agent.turn` capability call, counted and bounded like any other: `{"system", "messages", "tools", "tool_choice"}`, plus `"max_tokens"` when it is given. `messages` is the transcript as step 6 windows it. `tool_choice` is `"required"` with `submit` and `"auto"` without. The reply's data is `{"text", "tool_calls": [{"id", "name", "arguments"}]}`, appended as `{"role": "assistant", "content": text, "tool_calls": calls}`.
4. A reply without tool calls ends the loop with `{text}` when there is no `submit`. With `submit`, the engine appends the user message `Finish by calling submit.` and goes on.
5. The calls are handled in order, and each is answered with a tool message `{"role": "tool", "name", "tool_call_id", "content", "images"}`. `tool_call_id` is present only when the call had an id, and `images` only when there are pictures.
   - `submit`: the arguments are validated against the schema, and then passed to `agent.check` when `check` is true. An accepted value ends the loop at once with `{submitted}`; later calls in the same turn are left unanswered. A value the schema refuses is answered `refused: ` followed by `<where>: <message>` cut to 500 characters. `<message>` is the validator's message for the first error by location, and `<where>` is that location's segments joined by `/`, or `the answer` at the top. A value `agent.check` refuses is answered `refused: ` followed by the refusal cut to 500 characters.
   - An unknown name is answered `no tool named <name>`.
   - Any other name goes to `tool.invoke`. The content is `text(content)` ([identity.md](identity.md) §5), or the error text.
6. With `recent_images: n`, each request keeps only the newest n pictures across the transcript. A message that lost pictures has `\n[<k> older picture(s) not shown]` appended to its content.
7. After `max_steps` turns with no answer, the request fails with `agent_unfinished`: `the agent did not finish within <n> turns`.

The result is `{text}` or `{submitted}`, plus `transcript` (every message, unwindowed), `turns`, and `cost_usd` (the cost of the turns that were not cached).

### 6.3 `fact`

`{run_id, name, value}`: reports a node fact, a small value about the result, such as a score, a verdict or a measurement. `value` is any I-JSON value. The name `cost_usd` belongs to the engine, which always writes it (§5.3): a `fact` with that name, or with a value that holds a reserved marker (§3.2), is refused with `-32602`. Otherwise the engine records the fact at once, so it survives a later failure, and answers `{}`.

### 6.4 `annotate`

`{run_id, mark}`: adds one mark to the run's annotations. A mark has an optional `shape`, which is `point` (with `at: [x, y]`), `points` (`points: [[x, y], …]`, optionally `closed`) or `box` (`box: [x0, y0, x1, y1]`), in fractions of the image from 0 to 1. A mark without a shape is a note about the whole image. `label`, `color` and `tag` are optional strings, and other fields are kept as given. The engine refuses a malformed mark, or one holding a reserved marker (§3.2), with `-32602`, and otherwise answers `{}`.

### 6.5 `progress`

A notification, `{run_id, text, fraction?}`, with `fraction` from 0 to 1. The engine shows it.

### 6.6 `prompt.render`

`{run_id, path, variables}` → `{text}`. `path` MUST be one of the type's `resources`, exactly as declared (`undeclared_resource`). The engine decodes the file, removes its comments and renders it ([identity.md](identity.md) §5) over the run's params, with `variables` on top. Files in `variables` are file values. An unknown name, or a template that does not parse, fails with `expression_error`.

### 6.7 `file.put`

Stores a file the body made and returns its file ref, with its file facts. The params are `{run_id, kind?, name?}` plus exactly one source:
- `work_path`: a file under the run's `work_dir`, relative and POSIX. A path outside it, or one that names no file, is refused with `outside_work_dir`.
- `base64`: the bytes, base64 with padding (RFC 4648 §4).
- `json`: a JSON value, holding no reserved marker (§3.2). The engine writes the file in the format of [identity.md](identity.md) §5, "Writing JSON": the canonical form spread over lines. The same value is therefore the same file, and the same digest, in every language. Every SDK writes JSON outputs this way (Python: `ctx.out.json`).

`kind` defaults to the suffix rule for `work_path`, to `json` for `json`, and to `file` for `base64`. `name` is a display name.

## 7. Errors

Errors are JSON-RPC error objects, `{code, message, data?}`. `message` is a sentence for a person. JSON-RPC's own codes keep their meaning: `-32700` (parse error), `-32600` (invalid request), `-32601` (method not found) and `-32602` (invalid params). `internal` takes the place of `-32603`. FX's codes:

| Code | Name | Sent by | When | Node retried |
|---|---|---|---|---|
| -32000 | `node_failure` | host; engine for `agent.run` | The body failed the node on purpose, or a tool did | no |
| -32001 | `node_error` | host | The body or a check raised something unexpected; `data` has `exception` and `traceback` | only under `retry: engine`, at most 6 runs in all |
| -32002 | `cancelled` | both | The request was cancelled (§5.6), or its run was stopped | no |
| -32003 | `protocol_mismatch` | host for `initialize` | The engine's protocol is not the host's; `data` has both | session fails |
| -32004 | `load_failed` | host for `build`, `run` | A module, function or attribute could not be loaded | no |
| -32005 | `build_failed` | host for `build` | The builder raised, or returned no `Workflow` | no; a planning problem |
| -32010 | `capability_undeclared` | engine for `capability` | The type does not declare the capability in `calls` | no |
| -32011 | `over_bound` | engine for `capability`, `agent.run` | The run already made as many calls as declared | no |
| -32012 | `no_route` | engine for `capability`, `agent.run` | No route, or no adapter, serves the capability for this instance | no |
| -32013 | `not_live` | engine for `capability`, `agent.run` | The call is not cached and the run is not live | no |
| -32014 | `ceiling_exceeded` | engine for `capability`, `agent.run` | The call's hold does not fit the run's ceiling or a step budget | no |
| -32015 | `capability_refused` | engine for `capability`, `agent.run` | The adapter refused before sending anything; settled at $0 | no |
| -32016 | `call_failed` | engine for `capability`, `agent.run` | Every attempt failed, each one settled, or a collected job ended without a result | no: the engine already retried the call |
| -32017 | `job_unsettled` | engine for `capability`, `agent.run` | An earlier submission of this call has an unknown outcome | no |
| -32020 | `agent_unfinished` | engine for `agent.run` | No answer within `max_steps` turns | no |
| -32021 | `undeclared_resource` | engine for `prompt.render` | The path is not one of the type's resources | no |
| -32022 | `expression_error` | engine for `prompt.render` | The template names something nobody gave it, or does not parse | no |
| -32023 | `outside_work_dir` | engine for `file.put` | The `work_path` names nothing inside the work dir | no |
| -32024 | `unknown_file` | engine | A file value names a digest the engine did not hand this run | no |
| -32099 | `internal` | both | A fault in the engine or the host itself | no |

"Node retried" says whether the engine may run the body again when the error ends a run. The codes from `-32010` to `-32024` reach the body as exceptions it may catch; uncaught, they end the run with the same code. Capability errors carry `capability`, `route` and `key` in `data` when they are known.

## 8. The Python SDK (informative)

`grida.fx` keeps the `Ctx` authoring surface of FX's Python predecessor and rehosts it over the protocol:

| `Ctx` | Protocol |
|---|---|
| `ctx.instance`, `ctx.params`, `ctx.inputs` | `run`'s `instance`, except that the SDK sets `ctx.instance.take` to the last number of its `take` list (`take[-1]`); `params` with `param_files` put back; `inputs` as `InputFile` objects |
| `ctx.read.bytes/text/json/image` | local reads of a file ref's `path` |
| `InputFile.facts` | the file ref's file facts, `facts` |
| `ctx.out.json(v)` | `file.put` with `json` |
| `ctx.out.bytes`, `ctx.out.text`, `ctx.out.png` | `file.put` with `base64`, or a file under `work_dir` returned by `work_path` |
| `ctx.out.path(name)`, `ctx.work_path(name)`, `ctx.out.file(path)` | files under `work_dir`, returned by `work_path`; `ctx.out.file` of any other path goes through `file.put` with `base64` |
| `ctx.capability(…)`, `ctx.image_generate(…)`, … | `capability`; `CallResult` from the result |
| `ctx.agent(…).run(…)` | `agent.run`, with `tool.invoke` and `agent.check` coming back |
| `ctx.fact`, `ctx.annotate`, `ctx.progress` | `fact`, `annotate`, `progress` |
| `ctx.prompt(path, **vars)` | `prompt.render` |
| `ctx.tool(name).run(argv)` | a local process, using `run`'s `tools[name]` |
| `ctx.cancelled` | set by `$/cancel` |
| `ctx.fail(msg)` | `node_failure` |
| `ctx.state` | stays in the host, shared with the body's tools |

## 9. Example

A project `acme` with one node type:

```python
# nodes/caption.py
from grida.fx import Ctx, node


@node(
    "caption",
    inputs={"image": "image"},
    params={"question": {"type": "string", "default": "Describe the picture in one short sentence."}},
    outputs={"caption": "text"},
    calls={"structured.generate": 1},
    version=1,
)
async def caption(ctx: Ctx) -> dict:
    answer = await ctx.capability(
        "structured.generate",
        prompt=ctx.params["question"],
        context=[ctx.inputs["image"]],
        schema={"type": "object", "properties": {"caption": {"type": "string"}}, "required": ["caption"]},
    )
    text = answer.json["caption"]
    ctx.fact("words", len(text.split()))
    path = ctx.out.path("caption.txt")
    path.write_text(text + "\n", encoding="utf-8")
    return {"caption": ctx.out.file(path)}
```

The route is `llm-a@acme` (fingerprint `1a974f1107934b4388bf8dbbdc9931a2353db3612d5e2624c2f6eb3e6ba6db2e`). The input is a 136-byte, 64×64 opaque PNG. The first message on the wire is exactly:

```
Content-Length: 178\r\n
\r\n
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol":"fx-node-protocol-v1","engine":{"name":"grida-fx","version":"0.1.0"},"project_root":"/work/acme","sources":[]}}
```

The whole exchange is below, with the messages indented for reading. Both sides number their own requests from 1: the engine's `run` is its id 3, while the host's `capability` and `fact` are the host's ids 1 and 2.

```jsonc
// engine → host
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocol": "fx-node-protocol-v1",
    "engine": {"name": "grida-fx", "version": "0.1.0"},
    "project_root": "/work/acme",
    "sources": []
  }
}
// host → engine
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocol": "fx-node-protocol-v1",
    "host": {"language": "python", "version": "3.12.7", "sdk_version": "0.1.0a1"}
  }
}
// engine → host
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "describe",
  "params": {"targets": [{"path": "nodes/caption.py", "attribute": "caption"}], "builtins": false}
}
// host → engine
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "modules": [
      {
        "path": "nodes/caption.py",
        "types": [
          {
            "attribute": "caption",
            "spec": {
              "name": "caption",
              "inputs": {"image": "image"},
              "params": {
                "question": {
                  "type": "string",
                  "default": "Describe the picture in one short sentence."
                }
              },
              "outputs": {"caption": "text"},
              "judge": false,
              "calls": {"structured.generate": 1},
              "resources": [],
              "tools": [],
              "view": null,
              "version": 1,
              "retry": "service"
            }
          }
        ],
        "closure": [{"label": "nodes/caption.py", "path": "/work/acme/nodes/caption.py"}]
      }
    ],
    "builtins": []
  }
}
// engine → host
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "run",
  "params": {
    "run_id": "r1",
    "instance": {
      "id": "caption#1",
      "path": "caption",
      "step": "caption",
      "key": null,
      "take": [1]
    },
    "type": "nodes/caption.py#caption@1",
    "body": {"path": "nodes/caption.py", "attribute": "caption"},
    "params": {"question": "Describe the picture in one short sentence."},
    "param_files": {},
    "inputs": {
      "image": {
        "digest": "ad5e9999a3966063951fa4baef73a311c378ae034778afe41d450d98876c5859",
        "kind": "image/png",
        "size": 136,
        "name": "square.png",
        "path": "/work/acme/.fx/cache/files/ad/ad5e9999a3966063951fa4baef73a311c378ae034778afe41d450d98876c5859",
        "facts": {
          "bytes": 136,
          "kind": "image/png",
          "width": 64,
          "height": 64,
          "has_alpha": false,
          "opaque": true
        }
      }
    },
    "work_dir": "/work/acme/.fx/cache/work/r1",
    "resources": {},
    "tools": {},
    "calls": {"structured.generate": 1},
    "timeout_s": null
  }
}
// host → engine
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "capability",
  "params": {
    "run_id": "r1",
    "capability": "structured.generate",
    "request": {
      "prompt": "Describe the picture in one short sentence.",
      "context": [{"file": "ad5e9999a3966063951fa4baef73a311c378ae034778afe41d450d98876c5859"}],
      "schema": {
        "type": "object",
        "properties": {"caption": {"type": "string"}},
        "required": ["caption"]
      }
    }
  }
}
// engine → host
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "key": "cdea736cf1ae453c5995765ef83b58ef285443056586e2e4727ef10068f8c758",
    "cached": false,
    "cost_usd": 0.0012,
    "files": {},
    "data": {"caption": "A red square on a plain ground."}
  }
}
// host → engine
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "fact",
  "params": {"run_id": "r1", "name": "words", "value": 7}
}
// engine → host
{"jsonrpc": "2.0", "id": 2, "result": {}}
// host → engine
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {"outputs": {"caption": {"work_path": "out/caption.txt", "kind": "text/plain"}}}
}
// engine → host
{"jsonrpc": "2.0", "id": 4, "method": "shutdown"}
// host → engine
{"jsonrpc": "2.0", "id": 4, "result": null}
// engine → host
{"jsonrpc": "2.0", "method": "exit"}
```

The call key is `digest({"kind": "fx-call-v1", "capability": "structured.generate", "route": <fingerprint>, "request": <the request above>, "take": [1]})` ([identity.md](identity.md) §9). The engine then stores `out/caption.txt`, merges the node fact `words: 7` with the `cost_usd: 0.0012` it writes itself, and writes the result record under the step's identity.

## 10. Changes from the Python predecessor

The Python engine FX grew out of ran bodies in its own process and handed them live Python objects. Its protocol schema was never serialized.

| Predecessor | FX |
|---|---|
| A body's `Ctx` holds the engine's own objects | Everything crosses the protocol as data |
| Callbacks have no ids and no results | JSON-RPC requests in both directions, interleaved |
| The capability layer retries inside the provider adapter | The engine is the only retry owner, and it settles every attempt |
| Python runs the agent loop and its tools in one process | The engine runs the loop; tools and the check come back as `tool.invoke` and `agent.check` |
| Prompts are rendered in the host's Python | `prompt.render`: the engine owns the language and the resource check |
| `InputFile.facts` is computed lazily by a Python plugin | Every file ref carries the file facts the engine computed |
| `ctx.out.json` writes Python's `json.dumps(v, sort_keys=True, indent=1, ensure_ascii=False)` | `file.put` with `json`: the engine writes the format of [identity.md](identity.md) §5, the same bytes in every language, and the predecessor's bytes for most values |
| Tool results enter the transcript as Python `json.dumps` text | `text(v)` ([identity.md](identity.md) §5) |
| `ctx.progress` and `ctx.cancelled` do nothing | `progress` and `$/cancel` |
| A timeout cannot stop a body running in a thread | A host that ignores `$/cancel` is ended |
| A param holding a file that is neither text nor JSON arrives as a live object | `param_files` |
| `NodeFailure`, `CapabilityError` and other exceptions | Error codes, each saying whether the node may run again |
| A builder's takes file is `<builder module>.takes.yaml`, next to the builder file | `<workflow id>.takes.yaml` in the folder of the module that constructed the `Workflow` (§5.2), since one builder module may build several workflows. Milestone 2 renames such takes files. |
