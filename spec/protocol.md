# The node protocol

`fx-node-protocol-v1` · schema: [schemas/fx-node-protocol-v1.schema.json](schemas/fx-node-protocol-v1.schema.json)

The engine runs node bodies and workflow builders written in other languages through a **node host**: a separate process that loads the user's modules and runs their code. The Python host ships with `grida.fx`, and the TypeScript host with `@grida/fx`. This document is the contract between the engine and any host.

The split is fixed. **The host runs user code and nothing else.** The engine owns the store, the call cache, routes, prices, budgets, retries, prompt rendering, the agent loop, file facts and every identity. A host never computes a digest or a file fact and never writes the store. It talks to a provider only as an **adapter host** (§5.8), which runs a project's own adapter ([adapters.md](adapters.md) §9): the adapter reaches the network as it chooses, with its own client or through the engine's transport (`fetch`, §6.8), and the engine still keeps every hold, retry and record. The values a body reports about its own result with `fact` are *node facts* (§6.3), not file facts ([identity.md](identity.md) §4).

The words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

## 1. Transport

**Starting a host.** The engine starts the host with its working directory at the project root and talks over the host's stdin and stdout.
- A project module is hosted by the host of its language, chosen by suffix: `.py` is Python; `.ts`, `.mts`, `.js` and `.mjs` are TypeScript, all ES modules. A builder (§5.2) and a stand-in file (§5.7) choose their host the same way. Suffixes are compared exactly (`.TS` is not `.ts`). `.tsx`, `.cts` and `.cjs` modules are refused while planning with `<uses>: no node host for .<suffix> modules`, and so are declaration files, which hold types only (`<uses>: no node host for .d.ts modules`, or `.d.mts`); any other suffix with `<uses>: no node host for .<suffix> modules yet`, and a module without a suffix with `<uses>: no node host for modules without a suffix yet`. The built-in types whose bodies live in `grida.fx.std` are hosted by the Python host, so a workflow that uses one needs Python whatever the language of its own modules.
- A project, and one workflow, may hold modules of both languages, though one language per project is the norm. The engine keeps one planning host and one pool of hosts per language, and starts a language's host only when a module of that language needs it. A host that cannot start leaves the other language's modules unaffected.
- The Python host is `<python> -P -m grida.fx.host`. `<python>` is `GRIDA_FX_PYTHON` when it is set; otherwise the project's `.venv` interpreter (`.venv/bin/python`, or `.venv\Scripts\python.exe` on Windows) when it exists; otherwise the planning project's `.venv` interpreter ([store.md](store.md)) when that is another project and it exists; otherwise `GRIDA_FX_SDK_PYTHON` when it is set, the interpreter of an SDK that started the engine (§8.1); otherwise `python3` on `PATH`. An empty variable counts as unset. A node host's project is the workflow's home, so a workflow kept in a nested project of a monorepo finds the repository's `.venv` when it is run from the repository's root.
- **The Python host needs Python 3.11 or later,** for `-P` (below). Before the engine first starts an interpreter it chose as above, it asks the interpreter its version without `-P` (`<python> -c …`, standard input closed, standard error discarded), once per interpreter per command. An interpreter that answers a version before 3.11 is refused and the host is not started, so Python's usage text never reaches the user: the host is unavailable with `<python> is Python 3.9.6; FX's node host needs Python 3.11 or later with grida: run uv sync or pip install grida in the project, or set GRIDA_FX_PYTHON`, `<python>` named as the user configured it (`python3`, `.venv/bin/python`). An interpreter that cannot be asked, or does not answer within 10 seconds, is started as before. `grida-fx doctor` reports the same check on its Python line.
- `-P`, Python's safe-path option (Python 3.11 and later), keeps the working directory off `sys.path`. The project root is therefore not on `sys.path` until `initialize` puts it there, after the host has imported what it needs, and a project module named like a standard library module (`json.py`, `typing.py`) or like `grida` cannot replace the host's own imports. The engine also sets `PYTHONSAFEPATH=grida-fx` in the host's environment when the variable is unset or empty, for an interpreter wrapper that drops `-P`. The host removes a `PYTHONSAFEPATH` of exactly that value before it loads user code, so the programs user code starts inherit the user's environment.
- The TypeScript host is `<node> <host>`, with no other arguments. `<node>` is `GRIDA_FX_NODE` when it is set; otherwise `GRIDA_FX_SDK_NODE` when it is set, the Node running an SDK or a launcher that started the engine (§8.2); otherwise `node` on `PATH`. `<host>` is the `dist/host-entry.js` of an `@grida/fx`, looked for in this order:
  1. the one Node would resolve from the host's project: the one in the first folder, from the project root up, that holds `node_modules/@grida/fx/package.json` (a folder named `node_modules` is passed over, as Node passes it over). A node host's project is the workflow's home, as for Python, so a workflow in a nested project of a monorepo finds the `@grida/fx` installed at the repository's root;
  2. the one Node would resolve the same way from the planning project's root ([store.md](store.md)), when the planning project does not hold the home (a workflow kept in a `workflows:` folder outside it); a planning project that holds the home was walked in step 1;
  3. `GRIDA_FX_SDK_NODE_HOST` when it names a file: the host program of the `@grida/fx` that started the engine (§8.2).

  The first `@grida/fx` found decides, even one without a host program. An empty variable counts as unset. The host is unavailable with `the project's @grida/fx has no TypeScript node host: upgrade @grida/fx in the project` when the `@grida/fx` found has no `dist/host-entry.js`, and with `no @grida/fx for the TypeScript node host: run npm install @grida/fx in the project` when there is no host program at all. A message names a project's host program relative to the project, and never names the SDK's by its absolute path. Node's own messages may name files of the `@grida/fx` that holds the host program, so when that package's folder lies outside the home, the engine writes it as `@grida/fx` in every text it records or prints (`@grida/fx/dist/host.js`), and the host does the same for its own package's folder in every text it sends (§5.1).
- **The TypeScript host needs Node 24 or later with `module.registerHooks`,** for type stripping and for the source closure (§5.1). Before the engine first starts a Node it chose as above, it asks it for its version and whether it is Node.js with `module.registerHooks` (`<node> -e …`, standard input closed, standard error discarded), once per Node per command. The question is a script any Node can read, whether `-e` runs it as CommonJS or, under `NODE_OPTIONS=--input-type=module`, as an ES module: it takes `node:module` from `require` when there is one, else from `process.getBuiltinModule`. A Node that answers a version before 24 is refused and the host is not started: the host is unavailable with `<node> is Node 22.12.0; FX's TypeScript node host needs Node 24 or later: install it, or set GRIDA_FX_NODE`, `<node>` named as it was configured (`node`, `/opt/node/bin/node`; a `GRIDA_FX_SDK_NODE` by its absolute path, such as `/usr/local/bin/node`). A program that answers without the hooks (Bun, Deno, or a Node without them) is refused the same way, with `<node> is not Node.js 24 or later with module.registerHooks; FX's TypeScript node host needs it: install Node 24 or later, or set GRIDA_FX_NODE`. A Node that cannot be asked, or does not answer within 10 seconds, is started as before. `grida-fx doctor` reports the same check on its Node line. For its Node line, doctor starts the TypeScript host only for a project with a `package.json` (the home, or the planning project), since the host program may come from a folder above the project, which a project without one never asked for; for any other project that line says only what Node answered: `node      <node> (Node <version>)`, or `node      <node>` without an answer.
- A stand-in host (§5.7) is the host program of the stand-in file's language, found as above with the planning project as its project, and started in the engine's working directory.
- An adapter host (§5.8) is the host program of the adapter module's language, found as above with the planning project as its project, and started in the planning project's root. Its environment is FX's with only its own declaration's provider variables ([adapters.md](adapters.md) §9).

**Framing.** Each message is a header, `Content-Length: <n>\r\n`, an empty line `\r\n`, and then n bytes of UTF-8 JSON, as in LSP's base protocol. n counts bytes, not characters. A reader MUST ignore other header fields; a sender SHOULD write none.

**Messages.** Messages are JSON-RPC 2.0 requests, notifications and responses. Batches are not used. Every message MUST lie in the I-JSON domain ([identity.md](identity.md) §1): a sender never writes `NaN` or an infinity, and a receiver refuses a message that holds one with `-32700`. A host checks every value it sends against this domain, and refuses one outside it with a sentence that names where it is, rather than coerce it (§8.2).

**Both directions.** Both sides send requests and notifications.
- An id is an integer or a string, unique per sender for the session. A sender SHOULD count up from 1.
- The two sides' ids are independent. A message with `method` is a request (or notification) from its sender; a message without one answers a request the receiver sent.

**Interleaving.** Requests may be outstanding in both directions at once. While a `run` the engine sent is pending, the host sends `capability`, `fact` and the rest, and the engine sends `tool.invoke` and `agent.check` for the same run. While an adapter call the engine sent is pending, an adapter host sends `fetch` for it (§6.8). Each side MUST keep reading and answering while its own requests are pending. Responses may arrive in any order.

**One job per host.** A host handles one `describe`, `build`, `run` or `adapter.load` at a time: the engine sends the next only after the previous one is answered. To run nodes in parallel, the engine starts more hosts and reuses them. A builder changes the working directory, bodies share module state, and a run that has to be killed should take nothing else with it. Two hosts are exceptions, and neither is sent `describe`, `build` or `run`:
- a stand-in host serves one stand-in and nothing else, and may have several `stand_in.answer` requests pending at once (§5.7);
- an adapter host serves one project adapter and nothing else, and may have several `adapter.send`, `adapter.submit` and `adapter.collect` requests pending at once (§5.8).

**Output streams.** stdout carries protocol messages only. A host MUST keep whatever user code prints off it. Both hosts also keep user code from reading the protocol's input:
- The Python host duplicates file descriptor 1 for the protocol and points descriptor 1 at stderr before it imports any user code. It does the same with descriptor 0, which then reads `/dev/null`.
- The TypeScript host's first module imports only `node:fs`. Before any other module loads, it opens `/dev/fd/1` as the protocol's own descriptor, closes descriptor 1 and opens `/dev/fd/2`, which takes descriptor 1. It then opens `/dev/fd/0` as the protocol's input, closes descriptor 0 and opens `/dev/null`, which takes descriptor 0. A host that cannot open one of these anew, or does not get descriptor 1 or 0 back this way, refuses to start: it writes why on stderr (`@grida/fx node host: stderr could not become descriptor 1`) and exits with status 70. It never falls back to another descriptor. It then makes writes to `process.stdout` and `process.stderr` finish before they return when these are pipes, as Python's do: Node queues them otherwise (outside Linux), and the host ends without flushing them (below). This also clears `O_NONBLOCK` on the pipe, which every program user code starts shares. Code that a `NODE_OPTIONS` preload names (`--import`, `--require`) runs before the first module, outside this guarantee.

Either way, the processes user code starts do not inherit the protocol's own descriptors; `print`, `console.log`, `process.stdout` and those processes, with their streams inherited, write to stderr, and reading standard input gets nothing. stderr is free-form log text. The engine may show or keep it and never parses it.

A Python host inherits the engine's stderr. On Unix, the engine gives a TypeScript host a stderr pipe of its own instead, and copies what arrives on it to its own stderr until every process holding the pipe has closed it, or until the engine stops waiting for it (below). An inherited stderr would not do: the host opens it anew, and Linux cannot open a socket anew, while a regular file opened anew has an offset of its own, so the host's output and the engine's would overwrite each other; and Node, in the host or in a program user code starts, makes a pipe it writes to non-blocking, which would make the engine's own writes fail while a slow reader catches up.

The engine copies whole lines, each write made under its own lock on its stderr: complete lines, at most 512 bytes of them in one write (POSIX's smallest `PIPE_BUF`, which a pipe takes whole) or one longer line alone, and a line in parts only once 64 KiB of it is held, or when the pipe ends. The lines of TypeScript hosts that print at once, and the engine's own lines, therefore never splice, even when the engine's stdout and stderr share one pipe. An engine stderr that cannot take more now (one a parent process made non-blocking) is waited for until it can, and then given the rest. Any other failed write (the reader has gone) ends the copy's writing; the copy still reads its pipe to the end, so nothing that writes to the pipe blocks. The engine writes its own lines the same way, so a full non-blocking stderr never fails one, nor ends the work that prints it: it waits until the stream takes the line, as a blocking stderr would. A body's `progress` line (§6.5), which the engine prints as it reads the host's messages, holds that reading back meanwhile, and its run goes on once the line is taken. One of the engine's lines whose write fails otherwise is dropped.

When a TypeScript host has ended (it has exited and its group has been killed), the engine waits for its copy to write what the host left: what the copy has read by then, what its pipe holds then, as the system measures it (1 MiB where it cannot be measured), and one more read of 16 KiB the copy may be making. The wait is bounded by those bytes, not by time: it lasts however slowly the engine's stderr is read, and however late its reader starts (`2>&1 | less`, its first screen read at leisure), as a blocking stderr would. A process that left the host's group may hold the pipe open for good, so two things end the wait before the copy has ended: the copy has waited a second in all for more text on its pipe since the engine first waited for it (such a process holds the pipe idle, or prints now and then); or, once the copy has read more than the host left, 10 seconds have passed since then, however fast or slowly that text is read (such a process prints, perhaps faster than the engine's stderr is read, or into a stderr nobody reads). Before the command exits, the engine waits for every copy that has not ended by the same rule, on the same limits (a copy waited for before keeps the limits its first wait set). What a host wrote is therefore not lost when the command exits right after, however slowly or late its stderr is read; a command whose stderr nobody reads waits for it as it would on any full blocking stderr. Only a command that ends at once on an interruption may lose it. Such a command (a run's second Ctrl-C, or any interruption outside a run; [control.md](control.md)) waits for the copies by the same rule, 2 seconds at most from when it begins, whatever they do. Only then does it write its own last line, if it has one (`interrupted again: forcing exit; run completion and cleanup are unverified`): after what the hosts wrote, without the lock a copy given up may still hold, and with half a second at most for its stderr to take it. Such a command ends even when nobody reads its stderr. Its last line is lost only when its stderr cannot take it within that half second: a run whose cancellation completes meanwhile waits for it before the command exits. A process that still holds the pipe once the command has exited writes to a pipe that nobody reads, and those writes fail.

**End of input.** A host MUST exit when its stdin reaches end of file. Neither end of input nor `exit` waits for what user code started: the Python host flushes and closes the protocol stream, then ends the process at once (`os._exit`), whatever threads still run; the TypeScript host ends the process at once with Node's `process.reallyExit`, which runs no `exit` listener, so none can keep the host alive, whatever timers, sockets and child processes are left. `process.reallyExit` flushes nothing, and nothing is left to flush: the host writes the protocol stream and its own log lines synchronously, and user code's writes to `process.stdout` and `process.stderr` have finished (above). Before it exits, the TypeScript host kills the programs that bodies started with `ctx.tool` and that still run (§8.2). The TypeScript host also exits, with status 1, when a write to the engine fails (`could not write to the engine: …` on stderr) or a frame it reads is broken, even while the engine keeps its input open: it reads a pipe or a socket, as the engine gives it, in Node's event loop, never in a thread of Node's file-system pool, which Node's exit would wait for until the input ends. A file, and a named pipe on macOS (whose end of input Node's event loop never sees there), are read through the file system.

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
| `sources` | the project's declared source packages that the host's language can name (below), `[]` when there are none |

**Source packages.** A project's `sources` (`fx.yaml`) may hold Python package names (`^[A-Za-z_][A-Za-z0-9_]*$`, such as `acme_lib`) and npm package names (`^(?:@[a-z0-9][a-z0-9._~-]*/)?[a-z0-9][a-z0-9._~-]*$`, such as `acme-shapes` or `@acme/shapes`). The engine sends each host only the names its language can name: the Python host gets the names that match the first pattern, and the TypeScript host those that match the second, so a name such as `acme_lib`, which matches both, reaches both. A host refuses any other name with `-32602`, `sources is a list of package names`.

The result is `{protocol, host: {language, version, sdk_version}}`. `language` is `"python"` or `"typescript"`; the TypeScript host answers `"typescript"` for its `.js` and `.mjs` modules too. `version` is the language runtime's (Python's, or Node's `process.versions.node`, such as `24.14.0`) and `sdk_version` the SDK's (for Python, the `grida` distribution's; for TypeScript, the version of the `@grida/fx` that holds the host).

**Version mismatch.** A host that does not speak the engine's `protocol` answers with `protocol_mismatch`. Its message names both versions (`the engine speaks fx-node-protocol-v2 and @grida/fx 0.6.0 speaks fx-node-protocol-v1`), and its `data` is `{engine_protocol, host_protocol}`. An engine that gets back a `protocol` other than its own fails the session the same way. Either way the engine reports both versions and what to upgrade, for example `the project's grida 0.1.0a1 speaks fx-node-protocol-v1 and grida-fx 0.2.0 speaks fx-node-protocol-v2: upgrade grida`, or `the project's @grida/fx 0.6.0 speaks fx-node-protocol-v1 and grida-fx 0.7.0 speaks fx-node-protocol-v2: upgrade @grida/fx` for the TypeScript host, and then ends the host.

**`shutdown`** takes no params. The host stops accepting work and answers `null`. The engine then sends the **`exit`** notification, and the host exits with status 0, or with 1 if no `shutdown` came first. The engine MAY kill a host that has not exited 5 seconds after `exit`.

A host loads each module at most once per session. The engine starts a new session for each command it runs, so a session never outlives an edit it should have seen.

## 3. Shared values

### 3.1 File refs

Every file the engine hands a host is a **file ref**, except an adapter call's `files` (§5.8), which carry only `digest`, `kind`, `size` and `path`:

| Field | |
|---|---|
| `digest` | `file_digest` ([identity.md](identity.md) §2) |
| `kind` | the file kind ([identity.md](identity.md) §4) |
| `size` | bytes |
| `name` | a display name. It is never part of an identity. |
| `key` | the item's key in a keyed collection; absent or `null` otherwise |
| `path` | the absolute path of the file in the store. The host reads it and MUST NOT write, move or delete it. |
| `facts` | the file facts `facts(f)` ([identity.md](identity.md) §4, [facts.md](facts.md)), computed by the engine; for a file whose facts are refused, `bytes` and `kind` only |

The engine MUST include `facts` in every file ref it sends. When a file's facts are refused ([facts.md](facts.md) §1: a broken picture, an MP4 or Matroska file that does not parse), its ref still carries `facts`, holding only `bytes` and `kind`, and the engine logs the reason. Handing such a file to a host is not an error; an expression that reads its facts is ([facts.md](facts.md) §1). A host cannot tell such a file from one whose kind gives no further facts (an audio-only MP4) by its `facts`: a body that needs to know reads the file. A file ref is valid only within the run that received it.

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
| `description` | string, optional | the body's documentation (Python: its docstring; TypeScript: the declaration's `description`), for `grida-fx nodes`. Never part of an identity. |
| `inputs` | `{name: port}` | the files it reads |
| `params` | `{name: schema}` | the JSON values it is set with |
| `outputs` | `{name: port}` | the files it returns |
| `judge` | boolean | a judge reports the node fact `verdict`, `accept` or `reject` |
| `calls` | `{capability: number or param name}` | the most paid calls of each capability in one run |
| `resources` | `[path]` | project files the body reads at run time (prompt files, schemas): POSIX, relative to the project root, no `..` |
| `tools` | `[string]` | external programs: a name matching `^[a-z0-9][a-z0-9_-]*$`, optionally followed by a version bound that starts with one of `<>=!~`, as in `blender>=4.2` |
| `view` | string or null | a view template, kept for the views work (not in this version) |
| `version` | integer ≥ 0, or null | null for an unversioned type, whose identity is its export and its source ([identity.md](identity.md) §6) |
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

A param with neither `default` nor `x-fx-optional` is required, and a step that does not set it is refused while planning. SDK shorthand is expanded by the SDK: Python's `int` and TypeScript's `"integer"` become `{"type": "integer"}`; a tuple of choices in Python, and an array of strings in TypeScript (`["fast", "slow"]`), becomes `{"enum": […]}`; and in a schema, a boolean `optional` or `template` becomes `x-fx-optional` or `x-fx-template` (`optional: true` is `"x-fx-optional": true`).

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
- `modules` has one entry per target, in order: `{path, types: [{attribute, spec}], closure}`, or `{path, attribute?, error}` when the module fails to load, the attribute is missing, or it is not a node type. Errors read like `nodes/x.py failed to import: ModuleNotFoundError: No module named 'foo'`, or `nodes/x.ts failed to import: ERR_MODULE_NOT_FOUND: Cannot find module 'nodes/missing.ts' imported from nodes/x.ts`.
- `builtins` is `[{uses, spec}]`, such as `{"uses": "fx/image.resize@1", …}`. Every built-in spec has a `version`; its identity is `fx/<name>@<major>.<version>`.

**Node types in TypeScript.** A module-level name is an export of the module (`export const caption = node(…)`), and it holds a node type when its value carries `@grida/fx`'s node brand (§8.2). Without `attribute`, the host describes every such export in the order of the module's namespace, which sorts the names. It leaves out `default` and any name outside `[A-Za-z_][A-Za-z0-9_]*` (one with a `$`), which `uses` cannot name. With `attribute`, an export that is not a node type is the error `<path>: <attribute> is not an export declared with node()`, where the Python host says `<path>: <attribute> is not declared with @node`.

**Source closure.** `closure` lists the module and every project module it imports, transitively ([identity.md](identity.md) §6). Which files those are depends on the language's import rules, so the host computes the list; the engine reads and hashes the files itself, and computes the type identity and the `fx.lock` check. For Python:
- every `import a.b` in the file's syntax tree names `a.b`, and every `from m import x` names both `m` and `m.x`. Relative imports resolve against the importing file's package. Imports count wherever they appear (inside functions and conditionals too), whether or not they run.
- A name is in the closure when it resolves to `<base>/a/b.py` or `<base>/a/b/__init__.py`. The base is the project root, or, for a name whose first part is a declared source package, that package's parent folder.
- A file counts only when every part of its path below the base is spelled exactly as its directory entry, case included, as Python's own import requires. The closure is then the same on file systems that ignore case and on those that do not: `from Lib import Helper` names nothing when the file is `lib/helper.py`.

For TypeScript:
- The closure is the module and every source file (below) reachable from it along two kinds of edge, each from an importing file to a file it imports: the literal specifiers each source file holds, and the resolutions Node makes as the code runs.
- **Literal specifiers count whether or not the code that holds them runs,** as Python's rule counts every import. The host reads each `.ts`, `.mts`, `.cts`, `.js`, `.mjs` and `.cjs` source file's code, a TypeScript file's after Node's own type stripping, so a type-only import (`import type`, `typeof import("x")`) names nothing; a file of another suffix, or a TypeScript file that Node's type stripping refuses, names nothing. It finds `import … from "x"`, `import "x"`, `export … from "x"`, `import("x")` and `require("x")`, where `"x"` is a string literal or a template literal without substitutions, and skips comments, strings, template literals (whose `${…}` substitutions are code) and regular expression literals; a `require` that is a property (`a.require("x")`) is not one. It tells a regular expression from a division by the token before the `/`, as JavaScript tools usually do: after an operand (a name, a literal, `)`, `]`, `++`, `--`, or a keyword that is a property, such as `stats.in` or `opts.default`) a `/` divides, and after an operator, a punctuator, a keyword such as `return`, `typeof` or `in`, or the `)` that ends the head of an `if`, `while`, `for` or `with`, it starts a regular expression. A regular expression ends on its line, its body at most 65,536 characters long; a `/` that starts none is a division. The work is linear in the code. A computed specifier (`import(name)`, `` import(`./${name}.ts`) ``, `require("./" + name)`) is not a literal. Each literal resolves as Node would resolve it from the file: an import of a relative or absolute path, or of a `file:` URL, to exactly the file it names, since an ES module import adds no extension; a `#` subpath import or a package name by Node's lookup, with the conditions of an import; a `require` as `createRequire(<file>).resolve` resolves it. The file a literal names counts whatever its suffix (a `.json` module too), and a literal that resolves to no file, to a file that is not source, or to one spelled unlike its directory entries (below) names nothing. The files that literals name are read the same way whether or not Node ever loads them, each once per session, so the closure follows literals through code that never runs and does not depend on what ran.
- **Node's resolutions count too,** since only they know computed specifiers. The host records every `import` Node resolves, static or dynamic, with `module.registerHooks`, and every `require`, a CommonJS module's own and one made with `createRequire`, by wrapping `Module.prototype.require`, since Node answers a repeated `require` of one specifier from one folder from its own cache without calling the hooks. Each is an edge from the importing module to the file it resolved to: Node's own rules decide, whether an import is relative, a `#` subpath import (package.json `imports`) or a package name, and whatever the file's suffix (a project `.json` module counts too). Such an edge counts only once Node has loaded its file, so resolving alone adds nothing: `require.resolve` records no edge, and `import.meta.resolve` of a file that nothing loads adds nothing (but see the limits below).
- Which module a resolution's edge belongs to depends on when the import is made, never on which module Node loaded first; a literal's edge is always its own file's. A source module is *pending* while its top level runs. One that Node loads as an ES module is pending from the moment Node loads it until its top level ends, which the host learns from one call it appends to the module's source on a new last line, so the module's own lines and columns stay where they are; the call names a global function whose name the host draws at random when it starts, which no binding of the module can shadow. One that Node loads as CommonJS is pending until Node marks it `loaded`, or until it leaves `require.cache` (a module that deletes its own entry has finished). An edge made while its importer is pending is the importer's own, whichever module loaded the importer first: its static imports, and whatever its top level imports or requires. A `require` counts for the module whose `require` it is: a CommonJS module's own, or, through `createRequire`, the ES module whose URL made it; a `require` made by a `createRequire` from anything else (a folder, a `package.json`, a URL with a query) counts for the source module that calls it. An edge made later, by a function of a module whose top level has ended (a shared registry's lazy `import()`), is made at call time. It counts for each pending source module with a frame on the call stack, awaiting frames included, so a registry that another module's top level calls counts the file for that module only, never for every importer of the registry; when the stack shows none, for every pending ES module (a callback with no awaiting frame behind it); and when there is none either (a body or a tool runs), for the run whose async context made it, while that run is open (below). A module whose top level threw, or never ended, is taken as finished once the host's import, or the run, in which it started has ended. Describing modules in any order therefore gives each the same closure, but for the limits below.
- A source file is one whose real path lies under the project root, or under a declared source package's folder, with no `node_modules` folder between that base and the file. Anything else is not source: a dependency under `node_modules`, a Node built-in, a `data:` URL. Edges of either kind count only from source files, so a project file that only a dependency loads (a configuration loader's `import(url)` or `require`) is in no closure, whenever the dependency loads it; a type that reads such a file names it in `resources` (§4). A declared source package's folder is the one Node finds for its name from the project root (`node_modules/<name>/package.json`, in the root or a folder above it), taken by its real path, so a workspace package linked into `node_modules` counts where it really is. A name that finds nothing adds no base.
- A file counts only when every part of its path below its base is spelled exactly as its directory entry, case and Unicode normalization included. On a file system that ignores case, Node would load `lib/PNG.ts` for `lib/png.ts`; the host refuses the module instead, with `nodes/wrongcase.ts failed to import: lib/PNG.ts is spelled lib/png.ts on disk` (the label as imported, then as on disk). A name that differs from its entry only in Unicode normalization (NFC and NFD spell `é` differently) is refused with `<path> failed to import: <label> is spelled with a different Unicode normalization on disk`. The closure is then the same on every file system; where case matters, Node refuses such an import itself (`ERR_MODULE_NOT_FOUND`). A literal that names a misspelled file names nothing; when Node loads that file, the module is refused as above.
- A computed specifier cannot be known before it runs. So a host that runs bodies records a module's closure when it first loads the module, and checks every run against it. A run's own edges, and the source files it loads first, are those of the work in the run's async context while the run is open: the body and what it starts (promises, timers, callbacks), and what the host does for the run (the `tool.invoke` and `agent.check` requests it serves, the abort listeners a `$/cancel` runs). When a body has ended (returned or thrown), a source file fails the run when the first closure did not hold it, the module reaches it along its edges together with the run's own, and either Node first loaded it during this run or only the run's own edges reach it (a body's computed `import()` of a file another module had already loaded). The first such file that Node loaded, in label order, fails the node with `node_failure`, `the body loaded <label> at run time, outside its source closure: import it statically`, whatever the body returned or threw; or, when that file is spelled unlike its directory entries (above), `the body loaded <label> at run time, outside its source closure: <label> is spelled <on disk> on disk` (`… is spelled with a different Unicode normalization on disk`). A file that only a literal names, and that Node never loaded, is never the one named: the run reached it through a file that Node did load, which is outside the closure too. A literal `import()` that a body runs never fails it, since its file is in the closure. The run is then closed and its edges dropped. What it left running (a timer that imports later) counts for no run, and neither does work outside every run's async context (a timer or a server that a module started as it loaded), so a run never fails for what another run or another module loaded: only a run that itself loads such a file fails.
- A closure may still depend on what the session loaded before in five cases. `import.meta.resolve` of a file that another module does load counts once that file is loaded, since the resolve hook cannot tell it from an import. A computed specifier in a loader that memoizes what it loaded (a registry's cache, `codec ??= import(name)`) is resolved once, so it counts for the first module whose top level calls the loader, and fails a run only when that run is the first to call it in its host: write such specifiers as literals. A module whose top level outlives the host's import of it (an unawaited `import()` of a module whose top level awaits) is taken as finished when that import ends, so what it loads after that is made at call time. A callback with no awaiting frame behind it, run while several modules are pending (sibling modules whose top-level `await`s overlap), counts for each of them. And where the host cannot read the call stack (a frozen `Error`, as under `--frozen-intrinsics`), a call-time edge is counted as if the stack showed no pending module, and a `require` made by a `createRequire` from anything but a module records nothing.
- The hooks and the `require` wrapper see only the host's own thread. What a module or a body loads in a worker thread (`new Worker(…)`) or in a child process is in no closure, and the run-time check never sees it: a type whose code runs there names each of those files in `resources` (§4).

Each entry is `{label, path}`. `label` is the file's path, POSIX, relative to the nearer of its bases: the project root, or a declared source package, whose files are labelled with the package's name and their path inside the package (`acme_lib/colors.py`, or `@acme/shapes/src/index.ts` wherever that package's folder lies). `path` is the absolute path the engine reads; it never enters a record.

Loading a module runs its top-level code. A module outside the project root is an error. Whatever the module raises while it loads is its error; it never ends the host.
- In Python, imports resolve against the project root first, and `SystemExit`, `KeyboardInterrupt` and `asyncio.CancelledError` are errors like any other.
- In TypeScript, the host imports the module by its real path and Node resolves its imports, with its type stripping: only erasable syntax (`enum`, `namespace` and parameter properties fail with `ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`), imports that name the `.ts` file, and `#` subpath imports in place of tsconfig `paths`. A thrown value that is not an `Error`, and a rejected top-level `await`, are the module's error too. While user code may run, `process.exit` throws `process.exit(<code>) cannot end the node host`, and an exception or a rejection that nothing catches is logged on stderr and does not end the host.
- A TypeScript module that Node would load as CommonJS is refused: `<path> failed to import: <path> is not an ES module: set "type": "module" in package.json`. This holds for the module described or run, a builder file and a stand-in file, whether the host's import loads it first (it is refused before any of its code runs) or another module loaded it before (its code has run by then); their dependencies may be CommonJS, and another module may load a project file as CommonJS.
- In TypeScript, an `import` or a `require` of `@grida/fx` (or `@grida/fx/<subpath>`) that Node cannot resolve from the importing file resolves to the host's own package, so a project run with no `@grida/fx` of its own still loads. When the host's package cannot resolve it either (a subpath it does not export), the importer's own error stands. Type checking still needs the project's own.

An error names project files relative to the project root, never by an absolute path; the TypeScript host also cuts the root's `file:` URL, from stacks too, and names its own package's folder `@grida/fx` (`@grida/fx/package.json`), as path and as `file:` URL, wherever a text it sends names it. In TypeScript, an error reads `<code>: <message>` when it carries a Node error code (`ERR_…`), else `<name>: <message>`, or `<name>` alone when the message is empty. A thrown value that is not an `Error` reads `NonError: <the value as a string>`, and a `SyntaxError` in a project file ends with ` (<label>, line <n>)`. The TypeScript host carries no built-in types: it answers `builtins: []`, and a `run` of a built-in body fails with `load_failed`, `@grida/fx has no body for <builtin>`.

### 5.2 `build`

Runs a workflow builder (`grida-fx run workflows/levels.py:build`, or `workflows/levels.ts:build`). A builder target is `<file>:<function>`, where `<file>` has one of §1's suffixes and the host of its language runs it. A target whose file is `.tsx`, `.cts`, `.cjs` or a declaration file is refused with `<file>:<function>: no node host for .<suffix> modules`; with any other suffix, it is not a builder target.

| Param | |
|---|---|
| `path` | the builder file, relative to the project root |
| `function` | the builder function's name. In TypeScript, the builder is the module's export of that name. |
| `arguments` | `{name: string}`, given to the function as keyword arguments in Python, and as one frozen object of strings in TypeScript (`build({count: "3"})`) |
| `cwd` | the engine's working directory, absolute. The host loads the builder module (running its top-level code) and runs the builder there, since a builder reads its own files relative to where `grida-fx` runs, then returns to its own working directory. |

The result is `{document, takes_anchor}`:
- `document` is the workflow as `fx: workflow/v1`, exactly as the SDK writes it (Python's `Workflow`, TypeScript's `workflow()`), with no defaults filled in. The engine validates it like a workflow file. It enters the plan digest under `<path>:<function>` ([identity.md](identity.md) §10).
- `takes_anchor` is the project-relative path of the module that constructed the `Workflow` (in Python, the file that called `Workflow(…)`), or `path` when that module is unknown or outside the project. A TypeScript builder's anchor is always `path`, since JavaScript has no reliable caller frame. The engine keeps the workflow's takes file, `<workflow id>.takes.yaml`, in the anchor's folder.

Errors: `load_failed` (no such file or function, or an import failed) and `build_failed` (the builder raised anything, which in Python includes `KeyboardInterrupt` and `asyncio.CancelledError`, and in TypeScript a thrown value that is not an `Error` and a rejected promise; or it returned something other than a workflow: a `Workflow` in Python, and in TypeScript a plain object whose `fx` is `workflow/v1`, as `workflow()` makes, or a promise of one). A `build_failed` message reads `<function>: <exception type>: <message>`, or `<function>: <exception type>` when the message is empty. In TypeScript the rest is §5.1's error text (`build: TypeError: …`), and a value that is not a workflow reads `<function> returned <what it is>, not a workflow document` (`build returned an array, not a workflow document`).

### 5.3 `run`

Runs one instance's body. The engine sends `run` only for an instance that has to run: a result-cache hit, `fx/select@1` and the body-less built-ins never reach a host.

| Param | |
|---|---|
| `run_id` | a string unique in the session. Every host request made for this run carries it. |
| `instance` | `{id, path, step, key, take}`: the instance id ([identity.md](identity.md) §11), its step path with repeat keys, its declared step path, its repeat key or `null`, and its take: a list with one take number per regenerating level, outermost first, as in the step identity ([identity.md](identity.md) §8) |
| `type` | the type identity ([identity.md](identity.md) §6) |
| `body` | `{path, attribute}` for a project type, or `{builtin: "fx/<name>@<major>"}` for a built-in this host carries |
| `params` | the instance's param values. Templates are already rendered and defaults filled in. A text or JSON file given to a param arrives as its content: decoded text, or parsed JSON ([identity.md](identity.md) §5). A missing value arrives as `null`. A param the step left out and that has no default is absent. The engine never checks a value against its param's schema; the TypeScript host does before the body runs (§8.2). |
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
2. checks the result: no undeclared output port, every non-optional port present, each value's shape matching its port; no fact named `cost_usd`, which is the engine's (step 5), no reserved marker in a fact or a mark (§3.2), every mark well formed as `annotate` requires (§6.4), and no fact or mark nested deeper than 509 arrays and objects, which the run's record could not hold. A failed check fails the node, with `-32602` for a refused fact or mark, and it is not retried. A result FX cannot read at all fails the node with `the node host answered run with something FX cannot read: ` and the first place that is not what §3.4 describes (`output <port>: a file is {"work_path", "kind"?} or {"file": <ref>}`).
3. writes `{"kind": "fx-annotations-v1", "annotations": marks}` as the `annotations` output when the type declares one, the body returned none, and there are marks.
4. requires a judge's `verdict` node fact to be `accept` or `reject`.
5. stores the output files, sets the node fact `cost_usd`, and writes the result record (`fx-result-record-v1`) under the step identity when that is known. An output file under `work_dir` that cannot be read, or that changes while it is stored, fails the node: it is the body's, not the store's. The engine always writes `cost_usd` itself for a node a host ran: what this run paid for the node's calls, or `null` when it paid for none (a call answered from the cache costs nothing and counts as none; a stand-in's answer, §5.7, costs nothing and counts as paid, so a node it answered has `0`), the same value as the result record's `cost_usd` ([store.md](store.md) §3). `fx/select@1` and other engine types report no node facts.

**Failure.** The host answers `run` with an error:
- `node_failure` when the body failed on purpose (Python: `raise ctx.fail(…)` or `NodeFailure`; TypeScript: `throw ctx.fail(…)` or a `NodeFailure`), or, in TypeScript, was given a param value its schema refuses, before it ran (§8.2), or loaded a source file outside its module's closure (§5.1);
- `node_error` when it raised anything else. The error's `message` is the exception's own text, which may be empty, and `data` carries the exception's type and traceback. The engine names the failure `<exception>: <message>`, or `<exception>` alone when the message is empty. In TypeScript, the type is the thrown error's `name`, or `NonError` for a thrown value that is not an `Error`, whose text is the value as a string; the traceback is its `stack`, with project files named relative to the project root and the host's own frames and Node's internal frames left out;
- the error the engine answered one of the run's requests with, when the body let it propagate, with its code and message unchanged (`ceiling_exceeded`, `call_failed`, …).

For `node_failure` and `node_error`, `data` MAY carry node `facts` and `marks` the body kept locally, under the same rules as a result's. The engine merges them as above, leaving out any that step 2 would refuse, so a failed node keeps its node facts and its own error.

**Timeouts and cancellation.** At `timeout_s`, or when a person stops the run,
the engine sends `$/cancel` for the node's `run` request, and from then on answers
that request's host calls with `cancelled` (§6). A node timeout alone does not
cancel a paid call already sent: the engine retains that call and settles its
outcome. Cancelling the whole workflow invocation instead cancels local waiting
on in-flight sends/submission/collection and conservatively settles an unknown
received outcome at its full reservation. Resumable provider jobs retain their
`submitting`/`submitted` records as [store.md §5](store.md#5-long-jobs) specifies.
The runner waits for local call bookkeeping before recording cancellation; this
does not prove the provider stopped or confirmed the final bill. A forced process
exit can interrupt that bookkeeping. The [run-control contract](control.md)
defines acceptance, verified local completion and forced exit. First SIGINT/SIGTERM
and SDK run cancellation share its graceful path without an automatic runner
force timer; only a second explicit SIGINT grants emergency force authority.

A host that has not answered 5 seconds later is ended, together with what it
started where the platform allows that. Whatever a host started also ends with
the host whenever the host ends, not only after a cancel. A node that timed out
fails with `ran past <n> seconds`, whatever the host answers after the deadline
other than a result, and is not retried. A host that exits while a run is pending
fails that run as a `node_error` would, with `the node host exited with status <n>`,
`the node host was killed by signal <n> (<NAME>)` when a signal ended it, or
`the node host exited` when its status is not known.

### 5.4 `tool.invoke`

The engine calls one of the body's agent tools while the body's `agent.run` is pending.

| Param | |
|---|---|
| `run_id`, `agent_id` | the run, and the agent the body opened |
| `call_id` | the model's id for the tool call, or `null` |
| `name`, `arguments` | the tool and the JSON object the model filled in |

The host calls the tool: in Python, `function(ctx, **arguments)` for an `@tool` function, or `tool.handler(arguments)` for a declared tool; in TypeScript, the handler of a `tool(…)` declaration, as `handler(arguments, ctx)` (§8.2). The result is one of:
- `{"content": <JSON value>, "images": [file value, …]}`: the tool's answer. `images` is optional and holds pictures the host stored with `file.put` or received as file refs.
- `{"error": "<exception type>: <message>"}`: the tool raised (in TypeScript, threw anything but a `NodeFailure`, its type named as in §5.3). The model is told the text, and the loop goes on.

A tool that fails the node answers with a `node_failure` error. That ends the agent loop, and the engine answers `agent.run` with the same error.

### 5.5 `agent.check`

`{run_id, agent_id, value}`: the engine asks the body's check function about a submitted value that has already met the `submit` schema. The result `{"refusal": null}` accepts it. `{"refusal": "<why>"}` refuses it; in Python the check raised `ValueError` or `NodeFailure`, and in TypeScript it returned a string, or threw a `NodeFailure` (`throw ctx.fail(…)`). Any other exception is answered with `node_error`, which ends the agent loop like a tool's `node_failure`.

### 5.6 `$/cancel`

A notification, `{id}`, naming a pending request the engine sent. The host SHOULD stop that work and answer the request with `cancelled`. For a `run`, the Python SDK sets `ctx.cancelled`, which a body checks, and the TypeScript SDK aborts `ctx.signal`, an `AbortSignal` that a body checks or passes on (`fetch(url, { signal: ctx.signal })`), its reason an `Error` reading `the engine cancelled run`. The engine discards a result that arrives after it cancelled the request.

### 5.7 Stand-in answers

A **stand-in run** answers the paid calls that the call cache cannot answer with a function the caller supplies, the **stand-in**, instead of a provider. It is a test feature, like a mock model: the run is offline, every answer passes the checks a provider's answer passes (§6.1, *Stand-in answers*), nothing is billed or reserved, and what the run stores is kept apart from the project's store ([store.md](store.md) §8, *Stand-in runs*).

`grida-fx run --stand-in <source>` makes one. With `--live` or `--yes-up-to` it is a usage error (exit 2), `--stand-in cannot be used with --live` (or `--yes-up-to`): a stand-in run never spends, and it runs every phase. The engine reaches the stand-in through an **answerer**: a peer that speaks this protocol's transport (§1: framing, I-JSON messages, interleaving), to which only the engine sends requests. `<source>` is one of:
- `<file>#<function>`, where `<file>` has one of §1's suffixes (`.py`, `.ts`, `.mts`, `.js`, `.mjs`): the engine starts a **stand-in host** of the file's language (§1), outside the pools of node hosts, and loads the function with `stand_in.load`. `<file>` is relative to the working directory and may lie outside the project. The engine sends its absolute path to the host and records it nowhere.
- `-`: the answerer is already running, and the engine's standard input is a Unix-domain stream socket whose other end the answerer holds: one end of a socket pair an SDK made, or a connection an SDK accepted on a socket in a private folder it then removed (§8). The engine reads its standard input for nothing else, and every process the engine starts gets a standard input of its own, so no node host or tool inherits the socket.

Any other `<source>` is a usage error, `--stand-in takes <file>#<function> for a .py, .ts, .mts, .js or .mjs file, or -`, and so is a file that does not exist (`no stand-in file <file>`) or a standard input that is not a socket (`--stand-in - needs the stand-in's socket on standard input`). These are refused before anything is planned.

**Session.**

```
engine → answerer   initialize {protocol, engine, project_root, sources}  →  {protocol, host}
engine → answerer   stand_in.load {path, function}  →  {}              a stand-in host only
engine → answerer   stand_in.answer {…}  →  an answer, a decline, or an error; several at once
engine → answerer   $/cancel {id}       (notification)
engine → answerer   shutdown  →  null;  exit (notification)
```

- `initialize` is §2's, with the planning project's root and sources; a stand-in host gets the sources its language can name, as any host does, and an SDK's answerer on standard input does not read them. The engine starts the session before the run starts, which may be before planning. A session that cannot start, a version mismatch, or a stand-in that cannot be loaded ends the command with exit 2 and no run: `the stand-in <file>#<function> could not be loaded: <message>` (`the stand-in on standard input could not be started: <message>` for `-`).
- **`stand_in.load`**, `{path, function}`: `path` is the file's absolute path. The host loads the file as a module of its own, not as a project module (§5.1), so no closure is computed, and finds the function. The Python host puts the file's folder first on `sys.path`, as `python <file>` would have it, and the function may be a coroutine function. The TypeScript host imports the file by its absolute path, as `node <file>` would, so its imports resolve from its own folder (`@grida/fx` falling back to the host's own, §5.1); the function is the module's export of that name, and may be async. It answers `{}`, or `load_failed` with §5.1's wording (`<file> failed to import: …`, `<file> has no function <function>`). From then on the host answers `describe`, `build`, `run` and `adapter.load` with `-32600` (`this host answers for a stand-in`).
- The engine sends `shutdown` and `exit` when the run ends. An answerer that exits, or closes its end of the socket, while the run still runs stops the run: `the stand-in answerer exited`. The engine watches for this from the start, not only when it next asks: a run with steps still running stops at once, with a `problem` event `{where: "stand_in", message: "the stand-in answerer exited"}` unless a call met the loss first, and a pending `stand_in.answer` fails with that fault. A stand-in host is gone when its process exits, even while a process it started still holds the protocol pipes open: the engine reads what the host wrote before it exited for 5 seconds, then ends the connection.

**`stand_in.answer`** asks for the answer to one paid call:

| Param | |
|---|---|
| `capability` | the capability |
| `route` | `{id, fingerprint}` ([identity.md](identity.md) §7) |
| `request` | the canonical request ([identity.md](identity.md) §9), with each file as a file value (§3.2), exactly as it is keyed |
| `take` | the call's take list, as in the call key |
| `key` | the call key |
| `instance` | `{id, path, step}` of the instance making the call, as in `run` (§5.3) |
| `files` | `{digest: file ref}` (§3.1) for every file value anywhere in `request`, so the answerer can read them |

The result is one of:
- **An answer**, `{files, data}`. `files` maps each file's name to `{base64, kind?}`: its bytes, base64 with padding (RFC 4648 §4), and its kind ([identity.md](identity.md) §4). Without `kind`, a file takes the kind [capabilities.md](capabilities.md) gives its name in the capability's answer (§1 there); where that section gives none, `kind` is required. `data` is JSON, or `null`. An answer carries no cost: a stand-in run bills nothing.
- **A decline**, `{"decline": true}`: the stand-in does not answer this call, which goes on as in a run without a stand-in (`not_live`).

The answerer may instead answer with an error:
- `capability_refused`: the call is refused, as when an adapter refuses before sending;
- `call_failed`: the call failed, as when every attempt failed.

The error's `message` becomes the call's reason. Any other error, or a result FX cannot read, is a fault of the stand-in, not of the call: the engine fails the call with `internal` and stops the run, `the stand-in failed: <message>`, or `the stand-in answered something FX cannot read: <where>`.

**Concurrency and cancellation.** The engine sends one `stand_in.answer` per call it cannot answer from the cache (§6.1 step 3: one per key in flight), and several may be pending at once; an answerer may answer them in any order. When the instance making a call is cancelled (§5.3: a timeout, or a stopped run), the engine sends `$/cancel` for the request and fails the call with `cancelled` at once. It records nothing, and discards an answer that arrives later. The answerer SHOULD stop working on a cancelled request.

### 5.8 Adapters

A project's own adapter ([adapters.md](adapters.md) §9) is code in a project module that a node host runs. The planning host loads it while planning, so the engine knows its identity before any call; in a live run an **adapter host** loads it again and serves its calls.

```
engine → host   adapter.load {path, attribute}  →  {adapter, closure}      the planning host, an adapter host
engine → host   adapter.send {…}     →  an outcome                         an adapter host; several at once
engine → host   adapter.submit {…}   →  an outcome
engine → host   adapter.collect {…}  →  an outcome
host → engine   fetch {call_id, …}   →  a response, or a failure           while its call is pending (§6.8)
engine → host   $/cancel {id}        (notification)
```

**`adapter.load`**, `{path, attribute}`: `path` is the adapter's module, POSIX, relative to the project root, without `./` and without `..` segments (`adapters/acme.py`), and `attribute` the module-level name it is declared under; in TypeScript, an export of the module, named as `describe` names one (§5.1). The host loads the module as a project module, as `describe` does (§5.1): its top-level code runs, its imports resolve as a node module's, and an error names project files relative to the root. The result is `{adapter, closure}`:
- `adapter` is `{version, capabilities}`. `version` is an integer of at least 0, or `null` for an adapter without one. `capabilities` maps each capability the adapter serves, in the order it declares them, to `request` (answered with `adapter.send`) or `job` (`adapter.submit` and `adapter.collect`).
- `closure` is the module's source closure, `[{label, path}]`, found as §5.1 finds a node module's. The engine reads and hashes the files itself ([identity.md](identity.md) §6).

Errors: `load_failed`, worded as §5.1's: `<path> failed to import: …`, or, for a name that holds no adapter, `<path>: <attribute> is not declared with adapter()` (TypeScript: `<path>: <attribute> is not an export declared with adapter()`). A host whose SDK predates adapters answers `-32601`, and the engine names the SDK to upgrade ([adapters.md](adapters.md) §9). A stand-in host refuses `adapter.load` with `-32600` (§5.7), as does any host while a run is pending on it (`adapter.load came while a run is pending`). `adapter.load` changes nothing else a host serves: the planning host goes on describing modules and building workflows.

**Calls.** `adapter.send` asks the adapter to send a call's request once ([adapters.md](adapters.md) §3), `adapter.submit` to submit a long job once, and `adapter.collect` to collect a job by its handle (§6 there). Their params:

| Param | |
|---|---|
| `call_id` | a string unique in the session: the `fetch` requests made for this call carry it (§6.8) |
| `adapter` | `{path, attribute}`, as loaded. A host refuses a call for an adapter it has not loaded in this session with `-32602`. |
| `capability` | the capability: one the adapter declares, of the kind the method serves (`request` for `adapter.send`, `job` for the others). A host refuses another with `-32602`. |
| `route` | `{id, model, provider, contract}`: the route ([identity.md](identity.md) §7), with its contract as its table's reading completed it |
| `base_url` | the declaration's base URL, normalized: its variable's value when set, else its default ([providers.md](providers.md) §1) |
| `request` | the canonical request ([identity.md](identity.md) §9), with each file as a file value (§3.2), exactly as it is keyed |
| `files` | `{digest: {digest, kind, size, path}}` for every file value anywhere in `request`: its digest, kind and size, and the absolute path of its store copy, which the host reads and MUST NOT write, move or delete |
| `take` | the call's take list, as in the call key |
| `key` | the call key |
| `attempt` | the attempt, from 1 to 6 (§6.1 step 7) |
| `handle` | `adapter.collect` only: the job's handle, as `adapter.submit` accepted it |

The result is one **outcome**: an object whose `outcome` names it, with the members below. Its meaning, and what it bills, is [adapters.md](adapters.md) §3, §6 and §9.

| `outcome` | Members | Answers |
|---|---|---|
| `answered` | `files`, `data`, optionally `cost_usd` and `usage` | `adapter.send`, `adapter.collect` |
| `not_received` | `reason`, optionally `retry_after_s` | `adapter.send`, `adapter.submit` |
| `refused` | `reason` | `adapter.send`, `adapter.submit` |
| `failed` | `reason`, `retryable`, optionally `cost_usd` | `adapter.send`, `adapter.submit` |
| `accepted` | `handle` | `adapter.submit` |
| `uncertain` | `reason` | `adapter.submit` |
| `ended` | `reason`, optionally `cost_usd` | `adapter.collect` |
| `unreachable` | `reason` | `adapter.collect` |

- `files` maps each file's name to `{base64, kind?}`, and `data` is JSON or `null`, as in a stand-in's answer (§5.7).
- `cost_usd` is a number of at least 0: the cost the provider reported, in US dollars. Left out, it reported none.
- `usage` is `{input_tokens?, cached_input_tokens?, output_tokens?}`, each an integer of at least 0.
- `retry_after_s` is a number of seconds of at least 0: the wait the provider asked for.
- `reason` is a sentence ([adapters.md](adapters.md) §7), and `retryable` a boolean.
- `handle` is a JSON object: what collecting needs, and nothing secret ([adapters.md](adapters.md) §6).

An outcome its method does not take (an `accepted` send) is a result FX cannot read. The host answers `cancelled` after `$/cancel`; any other error, or a result FX cannot read, is a fault of the adapter, which ends its call as the loss of the adapter host would ([adapters.md](adapters.md) §9).

**Concurrency and cancellation.** An adapter host may have many calls pending at once, and answers them in any order. When the engine drops a call, it sends `$/cancel` for the call's request; the host SHOULD stop the adapter's work on it (in Python, cancel its task; in TypeScript, abort its `signal`) and answer `cancelled`. The engine answers the call's pending `fetch` requests with `cancelled`, and discards whatever the host answers later.

**The adapter host's session.** The engine sends `initialize` (§2) with the planning project's root and sources, then `adapter.load` for its adapter; it then sends calls, as a live run makes them, and `shutdown` and `exit` when the run ends. A host that exits, or closes its connection, fails the calls pending on it, and a call not yet written to it is not received ([adapters.md](adapters.md) §9).

## 6. Host to engine

Every request in this section carries the `run_id` of a pending `run`, except `fetch` (§6.8), which an adapter host sends for a pending adapter call: an adapter host has no run. A request with any other `run_id` is refused with `-32602`, and such a notification is ignored. When the engine stops a run, it answers that run's pending host requests with `cancelled`.

### 6.1 `capability`

A paid call: `{run_id, capability, request}`. `request` is the canonical request ([identity.md](identity.md) §9): JSON, with each file as a file value (§3.2). The engine:
1. checks that the type declares the capability (`capability_undeclared`) and has calls left (`over_bound`). Cache hits count toward the bound.
2. takes the instance's route for the capability (`no_route`).
3. checks the request's file values (§3.2) and computes the call key ([identity.md](identity.md) §9), then answers from the call cache when it can, billing nothing and removing a leftover job record with that key ([store.md](store.md) §5). This comes before the live check and the adapter check, so a dry run replays the calls it has already paid for, and a recorded call replays with no adapter at all: recorded calls replay offline ([store.md](store.md) §4). A key in flight is sent once: another call of the same key while it is in flight waits for it and gets its outcome as a cache hit, billing nothing. A key whose call ended `call_failed`, `capability_refused` or `job_unsettled` answers every later call of that key in the invocation with the same error and sends nothing, so a body run again under `retry: engine` never sends it again; `ceiling_exceeded` does the same for later calls from the same instance.
4. checks that the call can be sent. In a stand-in run (§5.7) the stand-in is asked first, as *Stand-in answers* below says: an answer, a refusal or a failure ends the call there, and only a decline goes on. The engine then refuses with `not_live` when the run is not live (a dry run, an `at: plan` step, or a stand-in run), or when the run has no ceiling and the route's provider bills in dollars: a live run without a ceiling runs on the user's own plan only ([providers.md](providers.md) §3), and every other call is refused. Then it refuses with `no_route` when no adapter serves the route: none of its capability and provider, or none under the `adapter` its contract names ([adapters.md](adapters.md) §1).
5. looks up the call's job record ([store.md](store.md) §5):
   - `submitting`: an earlier submission has an unknown outcome. The engine refuses with `job_unsettled`, and the message says to check the provider's dashboard and then run `grida-fx jobs --forget <key>`. When the record has a `note` ([store.md](store.md) §5), the error carries it as `reason` in `data`.
   - `submitted`: the engine collects the job by its `handle`, with no new hold and no new submit, because the run that submitted it was charged its hold. An answered job goes on at step 8. A job that ends without a result leaves its record `settled` and fails the call with `call_failed`; a later run submits it anew. An adapter that cannot collect jobs refuses with `job_unsettled`, as for `submitting`.
   - no record, or a `settled` one: the call goes on at step 6, as a new submission.
6. reserves, for each attempt, the route's high price for this request (on a capped route, its cap: [identity.md](identity.md) §12) under the run's ceiling and any step budget, $0 included, so every attempt is recorded (`ceiling_exceeded`, with `needed_usd` and `remaining_usd` in `data`). The attempt first takes its place under the route's concurrency and pacing, and only then reserves, so calls waiting for a place hold none of the ceiling. A reservation or a settlement the run's log cannot record stops the run.
7. makes the call as its only retry owner: at most 6 sends of the request in all, a resend after a send that provably was not received included; each attempt is reserved, recorded and settled. `capability_refused` means the call was refused before anything was sent, by the engine or the adapter ([adapters.md](adapters.md) §4), settled at $0. `call_failed` means every attempt failed. A long job keeps its job record as [store.md](store.md) §5 describes; a submission whose outcome is unknown is `job_unsettled`, with the adapter's redacted reason as `reason` in `data`, and a job that ends without a result fails the call with `call_failed`, its hold settled at the cost its adapter reports for the ended job, else in full ([adapters.md](adapters.md) §5). An answer is accepted only after these checks, in order, each of which fails the attempt as billed when it refuses: the adapter's own (for a project adapter, which judges inside its send and collect, first the answer's shape, as a stand-in answer's in step 3.1 below; [adapters.md](adapters.md) §9); for an adapter FX ships or a project adapter, the check [capabilities.md](capabilities.md) gives the capability for every route ([adapters.md](adapters.md) §3.4); a round trip of its data through the record's canonical JSON; and the engine's check of the capability (for `agent.turn`, §6.2).
8. stores the files and the call record (`fx-call-record-v1`), and then removes the call's job record, if any. The `data` and `files` handed back are the record's, as a replay reads them, so a live call and its replay give the body the same value.

The result is `{key, cached, cost_usd, files: {name: file ref}, data}`. `cost_usd` is 0 on a hit, for a collected job, which bills nothing new, and for a stand-in's answer, and `null` when the provider reported no cost (the engine then charged the whole hold).

**Stand-in answers.** In a stand-in run, the call cache of step 3 is the stand-in store ([store.md](store.md) §8, *Stand-in runs*), and step 4 asks the stand-in as follows. Steps 5 to 8 never apply: nothing is reserved, held, settled or retried, no job record is read or written, the route's concurrency and pacing do not apply, and a long job's capability is answered at once.
1. For a capability of [capabilities.md](capabilities.md), the engine first checks the request against it (§1 there; [adapters.md](adapters.md) §4 step 2). A refusal is `capability_refused`, `<capability> on <route> was refused: <reason>`, and the stand-in is not asked. The route's own refusals of values (adapters.md §4 steps 1, 3, 4 and 5) are not made.
2. It sends `stand_in.answer` (§5.7) and waits for the result, or for the instance to be cancelled. A decline goes on at the live check: `not_live`, `<capability> on <route> is a paid call the stand-in declined`. A `capability_refused` error ends the call with `capability_refused`, `<capability> on <route> was refused: <message>`, and a `call_failed` error with `call_failed`, `<capability> on <route> failed: <message>`.
3. It holds an answer to the checks of step 7, in this order. The first refusal fails the call with `call_failed`, `<capability> on <route> failed: <reason>`, and the call is not asked again:
   1. the answer's shape: the files and the `data` [capabilities.md](capabilities.md) §1 says an answer of the capability holds, refused with that section's sentences. For a capability it does not define, each file's kind is only checked to be a kind. Where it says the engine takes `data` from the answer's file (`video.generate`, `mesh.generate`), the engine writes `data` now;
   2. the route's check: the check of FX's adapter that would serve the route when FX has one ([adapters.md](adapters.md) §1, §3.4), which sends nothing; otherwise the check [capabilities.md](capabilities.md) gives the capability for every route (its *Every route's check* or *Check*, not *A route's check*), and none for a capability it does not define;
   3. the round trip of `data` through canonical JSON;
   4. the engine's check of the capability (`agent.turn`, §6.2).
4. It stores the files and then the call record, with `cost_usd` 0, in the stand-in store, and hands back the record's `files` and `data` as step 8 does, with `cached: false` and `cost_usd: 0`. The `call` event carries `stand_in: true`.

A call the stand-in refused or failed, or whose answer failed a check, ends its key for the invocation as step 3 says, and nothing of it is recorded: the next invocation asks again. A declined call is asked again whenever it is made.

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
3. Each turn is one `agent.turn` capability call, counted and bounded like any other: `{"system", "messages", "tools", "tool_choice"}`, plus `"max_tokens"` when it is given; never `provider_options`, so a turn takes only its route's ([adapters.md](adapters.md) §8). `messages` is the transcript as step 6 windows it. `tool_choice` is `"required"` with `submit` and `"auto"` without. The reply's data is `{"text", "tool_calls": [{"id", "name", "arguments"}]}`, with `arguments` an object. It is appended as `{"role": "assistant", "content": text, "tool_calls": calls}`, where each call's `arguments` is a string: the canonical JSON text ([identity.md](identity.md) §2) of the object the model gave, as provider APIs carry it. What the model wrote therefore never reads as a file value or a reserved marker (§3.2) in a later turn's request. Each call is dispatched with the object.
4. A reply without tool calls ends the loop with `{text}` when there is no `submit`. With `submit`, the engine appends the user message `Finish by calling submit.` and goes on.
5. The calls are handled in order, and each is answered with a tool message `{"role": "tool", "name", "tool_call_id", "content", "images"}`. `tool_call_id` is present only when the call had an id, and `images` only when there are pictures.
   - `submit`: the arguments are validated against the schema, and then passed to `agent.check` when `check` is true. An accepted value ends the loop at once with `{submitted}`; later calls in the same turn are left unanswered. A value the schema refuses is answered `refused: ` followed by `<where>: <message>` cut to 500 characters, for the first of the schema's errors. Errors are ordered by instance location (array indexes by number, member names by their text, and a location before the longer locations it starts), then by keyword location (the evaluation path, compared the same way). The errors one keyword reports at one place keep the schema's order (`required`). `<where>` is the instance location's segments joined by `/`, or `the answer` at the top. `<message>` is FX's own text for the failed keyword, from the keyword's value in the schema and the value found at that place, `<v>`, written as canonical JSON ([identity.md](identity.md) §2). `<n>` is canonical JSON, and character, item and property are plural unless n is 1:

     | Keyword | `<message>` |
     |---|---|
     | `type` | `<v> is not of type "<t>"`; several types sorted and joined by ` or ` |
     | `enum` | `<v> is not one of <enum>` |
     | `const` | `<v> is not <const>` |
     | `required`, `dependentRequired` | `"<name>" is a required property` |
     | `additionalProperties`, `unevaluatedProperties` | `the property "<a>" is not allowed` / `the properties "<a>", "<b>" are not allowed` (every refused member, canonical order) |
     | `minLength` / `maxLength` | `<v> is shorter than <n> characters` / `<v> is longer than <n> characters` |
     | `minimum` / `maximum` | `<v> is less than the minimum <n>` / `<v> is greater than the maximum <n>` |
     | `exclusiveMinimum` / `exclusiveMaximum` | `<v> is not greater than <n>` / `<v> is not less than <n>` |
     | `multipleOf` | `<v> is not a multiple of <n>` |
     | `minItems` / `maxItems`, `additionalItems` | `<v> has fewer than <n> items` / `<v> has more than <n> items` |
     | `unevaluatedItems` | `<v> has items that no schema allows` |
     | `uniqueItems` | `<v> has repeated items` |
     | `minProperties` / `maxProperties` | `<v> has fewer than <n> properties` / `<v> has more than <n> properties` |
     | `contains`, `minContains`, `maxContains` | `<v> does not hold the items contains asks for` |
     | `pattern` | `<v> does not match the pattern "<pattern>"` |
     | `format` (when asserted) | `<v> is not a valid "<format>"` |
     | `not` | `<v> matches the schema under not` |
     | `anyOf` | `<v> matches none of the schemas under anyOf` |
     | `oneOf` | `<v> matches none of the schemas under oneOf` / `<v> matches more than one of the schemas under oneOf` |
     | a `false` schema | `<v> is not allowed` |
     | `propertyNames` | `the property name ` followed by the message for the name |
     | any other | `<v> does not meet "<keyword>"` |

     A value `agent.check` refuses is answered `refused: ` followed by the refusal cut to 500 characters.
   - An unknown name is answered `no tool named <name>`.
   - Any other name goes to `tool.invoke`. The content is `text(content)` ([identity.md](identity.md) §5), or the error text.
6. With `recent_images: n`, each request keeps only the newest n pictures across the transcript. A message that lost pictures has `\n[<k> older picture(s) not shown]` appended to its content.
7. After `max_steps` turns with no answer, the request fails with `agent_unfinished`: `the agent did not finish within <n> turns`.

Before its first turn, the engine refuses an `agent.run` with `-32602` when `max_steps` is 0, a tool is named `submit`, a tool's `parameters` or the `submit` schema holds a reserved marker (§3.2), or `submit` is not a JSON Schema (draft 2020-12). The engine checks every `agent.turn` answer inside its retry owner (§6.1 step 7), before the call is recorded. Data that is not `{"text", "tool_calls"}` fails that attempt as billed, and the next attempt is a new one; when every attempt fails, the turn fails with `call_failed` and nothing is recorded. A missing or `null` `text` is `""`, missing or `null` `tool_calls` are none, a call without `arguments` has `{}`, and other members are not kept.

The result is `{text}` or `{submitted}`, plus `transcript` (every message, unwindowed), `turns`, and `cost_usd` (the cost of the turns that were not cached). An error that ends a loop that has started (anything but the refusals before the first turn) carries the transcript so far in `data.transcript`. Each `agent.run` starts a new transcript.

### 6.3 `fact`

`{run_id, name, value}`: reports a node fact, a small value about the result, such as a score, a verdict or a measurement. `value` is any I-JSON value. The name `cost_usd` belongs to the engine, which always writes it (§5.3): a `fact` with that name, with a value that holds a reserved marker (§3.2), or with a value nested deeper than 509 arrays and objects, which the run's record could not hold, is refused with `-32602`. Otherwise the engine records the fact at once, so it survives a later failure, and answers `{}`.

### 6.4 `annotate`

`{run_id, mark}`: adds one mark to the run's annotations. A mark has an optional `shape`, which is `point` (with `at: [x, y]`), `points` (`points: [[x, y], …]`, optionally `closed`) or `box` (`box: [x0, y0, x1, y1]`), in fractions of the image from 0 to 1. A mark without a shape is a note about the whole image. `label`, `color` and `tag` are optional strings, and other fields are kept as given. The engine refuses a malformed mark, one holding a reserved marker (§3.2), or one nested deeper than 509 arrays and objects, with `-32602`, and otherwise answers `{}`.

### 6.5 `progress`

A notification, `{run_id, text, fraction?}`, with `fraction` from 0 to 1. The engine shows it.

### 6.6 `prompt.render`

`{run_id, path, variables}` → `{text}`. `path` MUST be one of the type's `resources`, exactly as declared (`undeclared_resource`). The engine decodes the file, removes its comments and renders it ([identity.md](identity.md) §5) over the run's params, with `variables` on top. Files in `variables` are file values. An unknown name, or a template that does not parse, fails with `expression_error`.

### 6.7 `file.put`

Stores a file the body made and returns its file ref, with its file facts. The params are `{run_id, kind?, name?}` plus exactly one source:
- `work_path`: a file under the run's `work_dir`, relative and POSIX. A path outside it, or one that names no file, is refused with `outside_work_dir`.
- `base64`: the bytes, base64 with padding (RFC 4648 §4).
- `json`: a JSON value, holding no reserved marker (§3.2). The engine writes the file in the format of [identity.md](identity.md) §5, "Writing JSON": the canonical form spread over lines. The same value is therefore the same file, and the same digest, in every language. Every SDK writes JSON outputs this way (Python and TypeScript: `ctx.out.json`).

`kind` defaults to the suffix rule for `work_path`, to `json` for `json`, and to `file` for `base64`. `name` is a display name.

Params with no source or several are refused with `-32602`, `file.put takes exactly one of work_path, base64, json`. A `work_path` file that cannot be read, or that changes while it is stored, is refused with `-32602` as well: it is the body's doing, so the body may catch it and the run goes on. Only a store the engine cannot read or write stops the run.

### 6.8 `fetch`

An adapter host asks the engine to make one HTTP exchange for a pending adapter call (§5.8), through the engine's transport ([adapters.md](adapters.md) §2, §9). It carries the call's `call_id`, never a `run_id`.

| Param | |
|---|---|
| `call_id` | the `call_id` of a pending `adapter.send`, `adapter.submit` or `adapter.collect` |
| `method` | `GET` or `POST` |
| `url` | the absolute URL |
| `headers` | optional: `[[name, value], …]`, each name lowercase |
| `body` | optional, for a `POST`: `{base64, content_type}`, the bytes base64 with padding and their content type, or `{json}`, a JSON value sent as compact UTF-8 with `content-type: application/json` |
| `lane` | `provider`, `upload` or `download` ([adapters.md](adapters.md) §2 item 5) |
| `credential` | optional, `false` when left out: `true` asks the engine to attach the declaration's key |
| `timeout_s` | optional: the exchange's deadline in seconds, from connecting to the last byte, above 0 and at most 300, which is the default |
| `max_response_bytes` | optional: the most response body bytes read, from 1 to 67108864 (64 MiB), which is the default |

The engine sends the request once, never retries it and never follows a redirect. It attaches the key, as the declaration says, only on the provider and upload lanes, only when `credential` is `true`, and only to a URL whose origin (scheme, host and port) is the declaration's base URL's; a declaration without a credential sends none. It holds a download to the declaration's download hosts ([providers.md](providers.md) §1), and adds its URL to the redactor of the call's reasons. The result is one of:
- `{status, headers, base64}`: the response's status, its headers as `[[name, value], …]` with lowercase names, in order, and its body, base64 with padding;
- `{error: {phase, reason}}`: the exchange failed, `reason` saying why without a URL's path or query, a header value or a body. `phase` is `refused` when the engine or its transport refused the request before it left (the network is off, `GRIDA_FX_NETWORK=off`; a credential asked for on the download lane or for another origin, or one whose key is not set, `<VARIABLE> is not set`; a header named like a credential on the download lane, or the declaration's own credential header on any lane ([adapters.md](adapters.md) §2 item 5); a download outside the download hosts; a URL the transport cannot use); `not_sent` when it provably never left; and `after_send` otherwise. The adapter maps them onto its outcome as [adapters.md](adapters.md) §3.2 says.

A `call_id` that names no pending call, a `body` on a `GET`, and params outside the table above are refused with `-32602`. A fetch for a call the engine has dropped, or drops while the exchange is in flight, is answered `cancelled`.

## 7. Errors

Errors are JSON-RPC error objects, `{code, message, data?}`. `message` is a sentence for a person. JSON-RPC's own codes keep their meaning: `-32700` (parse error), `-32600` (invalid request), `-32601` (method not found) and `-32602` (invalid params). `internal` takes the place of `-32603`. FX's codes:

| Code | Name | Sent by | When | Node retried |
|---|---|---|---|---|
| -32000 | `node_failure` | host; engine for `agent.run` | The body failed the node on purpose, or a tool did; in TypeScript, also a param value its schema refuses, or a source file loaded outside the closure (§5.3) | no |
| -32001 | `node_error` | host | The body or a check raised something unexpected; `data` has `exception` and `traceback` | only under `retry: engine`, at most 6 runs in all |
| -32002 | `cancelled` | both | The request was cancelled (§5.6), its run was stopped, or the adapter call it serves was dropped (§5.8, §6.8) | no |
| -32003 | `protocol_mismatch` | host for `initialize` | The engine's protocol is not the host's; `data` has both | session fails |
| -32004 | `load_failed` | host for `build`, `run`, `stand_in.load`, `adapter.load` | A module, function, attribute or adapter could not be loaded | no |
| -32005 | `build_failed` | host for `build` | The builder raised, or returned no workflow | no; a planning problem |
| -32010 | `capability_undeclared` | engine for `capability`, `agent.run` | The type does not declare the capability in `calls` | no |
| -32011 | `over_bound` | engine for `capability`, `agent.run` | The run already made as many calls as declared | no |
| -32012 | `no_route` | engine for `capability`, `agent.run` | No route serves the capability for this instance, or no adapter serves the route of a call that must be sent | no |
| -32013 | `not_live` | engine for `capability`, `agent.run` | The call is not cached and the run is not live, a stand-in declined it (§5.7), or the run has no ceiling and the call is not on the user's own plan | no |
| -32014 | `ceiling_exceeded` | engine for `capability`, `agent.run` | The call's hold does not fit the run's ceiling or a step budget | no |
| -32015 | `capability_refused` | engine for `capability`, `agent.run`; a stand-in for `stand_in.answer` | The engine or the adapter refused the call before sending anything ([adapters.md](adapters.md) §4), settled at $0; or, in a stand-in run, the request does not fit its capability or the stand-in refused it | no |
| -32016 | `call_failed` | engine for `capability`, `agent.run`; a stand-in for `stand_in.answer` | Every attempt failed, each one settled, or a collected job ended without a result; or, in a stand-in run, the stand-in failed the call or its answer failed a check (§6.1, *Stand-in answers*) | no: the engine already retried the call, or a stand-in's answer is final |
| -32017 | `job_unsettled` | engine for `capability`, `agent.run` | An earlier submission of this call has an unknown outcome | no |
| -32020 | `agent_unfinished` | engine for `agent.run` | No answer within `max_steps` turns | no |
| -32021 | `undeclared_resource` | engine for `prompt.render` | The path is not one of the type's resources | no |
| -32022 | `expression_error` | engine for `prompt.render` | The template names something nobody gave it, or does not parse | no |
| -32023 | `outside_work_dir` | engine for `file.put` | The `work_path` names nothing inside the work dir | no |
| -32024 | `unknown_file` | engine | A file value names a digest the engine did not hand this run | no |
| -32099 | `internal` | both; a stand-in | A fault in the engine, the host, a stand-in or an adapter itself. A stand-in's stops the run (§5.7); an adapter's ends its call as the loss of its host would (§5.8) | no |

"Node retried" says whether the engine may run the body again when the error ends a run. The adapter methods (§5.8, §6.8) use no code of their own: an adapter's outcomes are results, an adapter host's faults are `internal`, and `fetch` reports a failed exchange in its result, as `tool.invoke` reports a tool's error. The codes from `-32010` to `-32024` reach the body as exceptions it may catch; uncaught, they end the run with the same code. Capability errors carry `capability`, `route` and `key` in `data` when they are known.

## 8. The SDKs (informative)

Each SDK hosts node bodies, builders, stand-ins and project adapters in its own language, over the same protocol: Python's `grida.fx` (§8.1) and TypeScript's `@grida/fx` (§8.2).

### 8.1 Python

`grida.fx` offers the `Ctx` authoring surface over the protocol:

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

**The interpreter.** `grida.fx.plan`, `run` and their async forms, and `python -m grida.fx`, start the engine with the caller's environment plus `GRIDA_FX_SDK_PYTHON` set to the interpreter running the SDK (`sys.executable`). That interpreter has `grida`, which node hosts and stand-in hosts need, so a project without a `.venv` does not fall back to a bare `python3`; it comes after `GRIDA_FX_PYTHON` and every project `.venv` (§1), so neither the caller's choice nor a project's own environment is overridden.

**Stand-ins.** `grida.fx.run(…, stand_in=answer)` and `run_async` serve `answer` in the caller's own process: they make a Unix-domain socket pair, give the engine one end as its standard input with `--stand-in -`, and answer `initialize`, `stand_in.answer`, `$/cancel`, `shutdown` and `exit` on the other (§5.7). The function's state (counters, scripted answers, the requests it saw) is therefore the caller's, and each invocation may get another function. `grida-fx run --stand-in file.py#answer` serves the same function from a stand-in host.

| Python | Protocol |
|---|---|
| `answer(call)`, a function or a coroutine function of one `StandInCall` | `stand_in.answer`; the SDK answers one call at a time, in the order the requests arrive, a plain function on a worker thread |
| `call.capability`, `call.route.id`, `call.route.fingerprint`, `call.key`, `call.instance.id`/`.path`/`.step` | the params of the same names |
| `call.takes`, `call.take` | `take`, and its last number (`take[-1]`), as for `ctx.instance` |
| `call.request` | `request`, with every file value replaced by an `InputFile` built from `files` |
| `call.files` | `files`: `InputFile` objects by digest |
| `return Answer(files={name: bytes or Output or InputFile or Path}, data=…)` | an answer; bytes leave `kind` out, an `Output` or an `InputFile` gives its kind, a `Path` the suffix rule's |
| `Answer.json(value)`, `Answer.turn(text, tool_calls)`, `Answer.submit(**arguments)` | `data` of `{"json": value}`, of an `agent.turn`, and of an `agent.turn` that calls `submit` |
| `return DECLINE` | `{"decline": true}`; returning `None` is a fault of the stand-in |
| `raise CallRefused(msg)`, `raise CallFailed(msg)` | `capability_refused`, `call_failed` |
| any other exception | `internal`: the engine stops the run, and `run` raises that exception once the engine has ended |

**Adapters.** A project adapter ([adapters.md](adapters.md) §9) is declared at a module's top level with `adapter(…)`, whose value the host recognises. The host serves it as §5.8 says: the planning host answers `adapter.load`, and an adapter host its calls too.

| Python | Protocol |
|---|---|
| `acme = adapter(requests=["image.generate"], jobs=["video.generate"], send=…, submit=…, collect=…, version=1)` | `adapter.load`'s `adapter`: each capability of `requests` is a `request` and each of `jobs` a `job`, in that order; `version` is an integer of at least 0, or `None`, the default. `adapter()` refuses, with a `ValueError` or a `TypeError` where the module makes it, a declaration with no capability, a name that is not a capability's, a capability named twice, `requests` without `send`, `jobs` without both `submit` and `collect`, a function that cannot be called, or another version; the module then fails to import (§5.1) |
| `send(call)`, `submit(call)`, `collect(call)`, each a function or a coroutine function of one `AdapterCall` | `adapter.send`, `adapter.submit`, `adapter.collect`. The SDK answers calls concurrently, in any order: a coroutine function as a task on the host's loop, a plain function on a worker thread of its own |
| `call.capability`, `call.route.id`/`.model`/`.provider`/`.contract`, `call.base_url`, `call.key`, `call.attempt`, `call.handle` | the params of the same names; `call.handle` is `None` outside `collect` |
| `call.takes`, `call.take` | `take`, and its last number (`take[-1]`), as for `ctx.instance` |
| `call.request` | `request`, with every file value replaced by an `AdapterFile` built from `files` |
| `call.files` | `files`: `AdapterFile` objects by digest, with `digest`, `kind`, `size`, `path` and `read_bytes()` |
| `await call.fetch(url, *, method="GET", headers=None, json=None, body=None, content_type=None, lane="provider", credential=False, timeout_s=None, max_response_bytes=None)` | `fetch`, carrying the call's `call_id`: a `FetchResponse` (`status`; `headers`, `(name, value)` pairs in order; `body`, bytes; `.json()`, `.text()`), or a `FetchError` raised with the failure's `phase` and `reason`. `headers` is a mapping or pairs, its names lowercased. What the engine would refuse with `-32602` (a body on a `GET`, `timeout_s` or `max_response_bytes` out of range, a name that is not a token), and `json` with `body`, raise a `ValueError` or a `TypeError` before anything is sent; an error answer raises the `EngineError` of its code (`Cancelled` for a call the engine dropped). `fetch` is a coroutine: a plain function awaits it on a loop of its own (`asyncio.run`) |
| `return Answered(files={name: bytes or Output or AdapterFile or Path}, data=…, cost_usd=…, usage=…)`, `Answered.json(value, …)` | `answered`; its files as a stand-in's `Answer` takes them, but an `AdapterFile`, with its kind, where a stand-in's takes an `InputFile`; `usage` a mapping of the token counts |
| `NotReceived(reason, retry_after_s=…)`, `Refused(reason)`, `Failed(reason, retryable=…, cost_usd=…)` | `not_received`, `refused`, `failed`; `retryable` is required. Each outcome holds its members when it is made, with a `ValueError` or a `TypeError`: a blank `reason`, a `cost_usd` or a `retry_after_s` that is not a finite number of at least 0, a `usage` count that is not an integer of at least 0, a handle that is not a mapping |
| `Accepted(handle)`, `Uncertain(reason)` | `accepted`, `uncertain`; a handle is a mapping of JSON values |
| `Ended(reason, cost_usd=…)`, `Unreachable(reason)` | `ended`, `unreachable` |
| `$/cancel` | answers the call `cancelled` at once and cancels a coroutine function's task; a plain function runs to its end, and what it returns is discarded |
| any exception, or a value that is not an outcome its method takes | `internal`, `<Type>: <message>`, its traceback on stderr: the engine ends the call as the loss of its host would. A fault ends only its own call |

### 8.2 TypeScript

`@grida/fx` carries the TypeScript host, `dist/host-entry.js`, and its authoring surface. It is written in TypeScript and shipped as JavaScript and `.d.ts` files, never `.ts`, because Node refuses to strip types under `node_modules`: the protocol glue is plain JavaScript at run time, and all typing lives in the `.d.ts`. A project's own `.ts` modules run as they are, with Node's type stripping (§5.1), and its `.js` and `.mjs` modules are hosted the same way. Hosting needs Node 24 or later (§1); the rest of the SDK keeps its own, lower requirement.

**Declarations are values.** `node(declaration, body)` declares a node type with the type spec's names (§4): `name`, `description`, `inputs`, `params`, `outputs`, `judge`, `calls`, `resources`, `tools`, `view`, `version` and `retry`. The declaration is checked when it is made, with the Python SDK's sentences, and the node type carries the brand `Symbol.for("grida.fx.node")`. A member whose value is `undefined` is left out, in the declaration and in its ports, params, calls and schemas, so `{ type: "integer", default: undefined }` has no default; a field `node()` does not know is a compile error, and an error when the declaration is made. Its static types are inferred from the declaration through `const` type parameters, with no code generation and no reflection at run time:
- an input of one file is an `InputFile`, a list (`kind[]`) a readonly array, and a keyed collection (`kind{}`) a `ReadonlyMap` in collection order, never a plain object, which would put integer-like keys first (`2`, `10`, `sun` rather than `sun`, `10`, `2`). An optional port is an optional property.
- a param declared `"string"` is a `string`, `"integer"` and `"number"` are `number`s, `"boolean"` is a `boolean`, `"array"` is a `readonly JsonValue[]` and `"object"` a `{ readonly [key: string]: JsonValue }`, an array of choices is their union, and a schema is the type its `enum`, `const`, `type`, `prefixItems`, `items` and `properties` say: `prefixItems` a tuple whose items may each be missing, followed by the items `items` describes (none for `items: false`). An array or object schema that says no more, and a schema with none of these keywords, are JSON values (`JsonValue`) too, so a body can pass such a param on to a request, a fact or `ctx.out.json` as it is. An optional param (`optional: true`), with a `default` or without, is an optional property, since a step that binds it to `null` leaves it out. The host checks each value against its schema before the body runs (*Params are checked*, below), so these types hold when the body reads them.
- the body returns exactly the declared outputs, none missing and none undeclared: for a file, an `Output` that `ctx.out` made or an `InputFile`; for a list, an array of them; for a keyed port, a `Map`, sent in its order. A plain object on a keyed port fails the node with `output <port> is an object; a keyed output is a Map`. An optional output may be left out or `null`.

| `Ctx` | Protocol |
|---|---|
| `ctx.instance`, `ctx.params`, `ctx.inputs` | `run`'s `instance`, with its `take` list as `ctx.instance.takes` and the list's last number as `ctx.instance.take`; `params` with `param_files` put back as `InputFile` objects; `inputs` as `InputFile` objects, in readonly arrays and `ReadonlyMap`s |
| `ctx.read.bytes/text/json`, `InputFile.bytes/text/json` | local reads of a file ref's `path`. There is no image decoding: a body brings its own library. `ctx.read` takes an `InputFile`, or the name of an input of one file that is not optional; an input that is absent anyway fails the node with `input <name> is absent` |
| `InputFile.facts` | the file ref's file facts, `facts` |
| `ctx.out.json(v)` | `file.put` with `json` |
| `ctx.out.bytes`, `ctx.out.text`, `ctx.out.png` | `file.put` with `base64`, or a file under `work_dir` returned by `work_path`; `ctx.out.png` takes encoded PNG bytes |
| `ctx.out.path(name)`, `ctx.workPath(name)`, `ctx.out.file(path)` | files under `work_dir`, returned by `work_path`; `ctx.out.file` of any other path goes through `file.put` with `base64` |
| `ctx.capability(…)`, `ctx.imageGenerate(…)`, … | `capability`; `CallResult` from the result |
| `ctx.agent(…).run(…)` | `agent.run`, with `tool.invoke` and `agent.check` coming back |
| `ctx.fact`, `ctx.annotate`, `ctx.progress` | `fact`, `annotate`, `progress` |
| `ctx.prompt(path, variables)` | `prompt.render` |
| `ctx.tool(name).run(argv)` | a local process, using `run`'s `tools[name]`, in a session of its own (below) |
| `ctx.signal` | an `AbortSignal`, aborted by `$/cancel` |
| `ctx.fail(msg)` | `node_failure` |
| `ctx.state` | stays in the host, shared with the body's tools |

**Params are checked.** The engine never checks a param's value against its schema (§5.3): a step's `with:` may bind an expression, and a missing value arrives as `null`. So before the body runs, the host goes through the declared params that arrived, in the order they are declared:
- a `null` for an optional param (`x-fx-optional`) means the param is absent: the body does not receive it, as when the step left it out;
- any other value is checked against the param's JSON Schema (draft 2020-12). The check knows `type` (a list of types, `integer` and `null` included), `enum`, `const`, `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`, `minLength`, `maxLength`, `pattern`, `items`, `prefixItems`, `minItems`, `maxItems`, `uniqueItems`, `properties`, `patternProperties`, `required`, `additionalProperties` (a boolean or a schema), `minProperties` and `maxProperties`, and passes over any other keyword.

A value at a `param_files` pointer (a file) is not checked itself; the keywords of the places that hold it (`enum`, `const`, `uniqueItems`) see it as its plain projection, `{"file": "<digest>"}` (§3.2), so two different files are not repeated items. `multipleOf` is exact in decimal: `0.7` is a multiple of `0.1`. `items: false`, after any `prefixItems`, is a `false` schema for each further item, which is refused there with `<v> is not allowed`. A `pattern`, and a `patternProperties` name, is an ECMA-262 regular expression, read with the `u` flag when it is one under that flag and without it otherwise (`^\d{3}\-\d{4}$`, whose `\-` the `u` flag refuses); `node()` refuses a declaration whose param schema holds one that is neither, wherever the check would read it: `<name>: param '<param>': the pattern '<pattern>' is not a regular expression`.

The first value its schema refuses fails the node with `node_failure`, and the body never runs: `param <name>: <message>`, or `param <name>/<path>: <message>` for a place below the param's top, `<path>` its segments joined by `/`. `<message>` is the first of the value's errors in §6.2's order, worded as §6.2's table words it: `param side: "up" is not one of ["left","right"]`, `param size: "big" is not of type "integer"`. The Python host checks no param values.

**Programs.** `ctx.tool(name).run(argv, { cwd, timeoutS, env, check })` starts the program in a session of its own, so its process group holds what it starts. The group is killed at `timeoutS` seconds, which fails the node with `<name> ran past <s> seconds`; when `ctx.signal` aborts, which rejects the call with the signal's reason; once the run's answer is sent, for a program the body left running; and when the host ends, or finds the engine that started it gone. `timeoutS` is a number of seconds above 0, or `null` or left out for none; any other value rejects with a `TypeError`, `timeoutS is a number of seconds above 0, or null, not <value>`. A timeout longer than Node's longest timer still waits its whole time.

Bodies, tools and checks run on the host's one event loop, so a body that never yields holds up `$/cancel` until it returns; a host that does not answer in time is ended (§5.3).

**Tools and agents.** `tool({ name, description, parameters }, handler)` declares an agent tool, with the brand `Symbol.for("grida.fx.tool")`. `parameters` is a JSON Schema value; written `as const`, it gives the handler's arguments their type, with no validator at run time. A `parameters` that does not describe an object types them as an object of JSON values, which is what the engine passes. The host calls `handler(arguments, ctx)` (§5.4). A JSON value the handler returns is the tool's `content`, `new ToolReply(text, images)` is `{content: text, images}`, and any other value is sent as its string. A `NodeFailure` the handler throws fails the node; anything else it throws is told to the model. An agent's `check` returns a string to refuse a submitted value and nothing to accept it. A `NodeFailure` it throws is a refusal with its message, and anything else it throws ends the loop with `node_error` (§5.5).

**Errors.** `NodeFailure` fails the node on purpose. `EngineError` carries the `code`, `message` and `data` of an engine error the body may catch, with a subclass per code, as in Python: `Cancelled`, `CapabilityUndeclared`, `OverBound`, `NoRoute`, `NotLive`, `CeilingExceeded`, `CallRefused`, `CallFailed`, `JobUnsettled`, `AgentUnfinished`, `UndeclaredResource`, `ExpressionError`, `OutsideWorkDir` and `UnknownFile`. The host recognises node types, tools, outputs and these errors by `Symbol.for` brands, so a module that imports another copy of `@grida/fx` still works.

**Values.** `JSON.stringify` would write `NaN` as `null`, write `undefined` in an array as `null` and write a `Map` as `{}`. The host therefore checks every value it sends (§1), and refuses one with a sentence that names where it is, such as `at /value: NaN is not a JSON number` (a fact's refusal names the fact first, `fact <name>: …`). It refuses:
- `NaN` and the infinities;
- integers beyond ±2^53, such as `2 ** 60`, with `the integer <n> cannot be read exactly`. [identity.md](identity.md) §1 reads a larger integer that is written exactly, but a JavaScript number past 2^53 has usually been rounded already, so the TypeScript host sends none;
- `undefined` as a whole value or as an array item (`undefined is not a JSON value`), functions, symbols and `BigInt` values;
- lone surrogates, in strings and in keys;
- objects that are not plain, such as a `Map` (`a Map is not a JSON value`), a `Date` or a class instance; a value that holds itself; and values nested deeper than the engine reads.

An object member whose value is `undefined` is left out, as `JSON.stringify` and `toYaml` leave it out, wherever the SDK sends or writes a value: a request (`{ size: ctx.params.size }` sends `{}` without the param), a mark, a fact, a builder's document, a stand-in's answer. The SDK's types say the same: a JSON value it takes is a `JsonInput`, whose arrays and objects may be readonly (`as const`, a submitted value, `ctx.instance.takes`) and whose object members may be `undefined`, while a JSON value it hands out is a `JsonValue`. A capability request and a prompt's variables are a `RequestObject`: `JsonInput`, with an `InputFile` or an `Output` anywhere in it, sent as a file value (§3.2).

Object params arrive as JavaScript objects, which put integer-like keys first; only keyed collections keep their order, as `ReadonlyMap`s.

**Builders** are exported functions, sync or async, that return `workflow(…)`'s document. A builder is called with one argument, a frozen object of the `--arg` strings (`build({count: "3"})`), and its takes anchor is its own path (§5.2).

**The runtime.** `@grida/fx`'s calls that start the engine, and its launcher (`grida-fx`, and `grida fx` through `@grida/fx/cli`), start it with the caller's environment plus:
- `GRIDA_FX_SDK_NODE`, the Node running them (`process.execPath`), only when that is Node: not under Bun or Deno;
- `GRIDA_FX_SDK_NODE_HOST`, the absolute path of their own host program (`dist/host-entry.js`), when that file exists.

An `env` that a caller passes to an SDK call still wins. Both come after `GRIDA_FX_NODE` and after the `@grida/fx` of the workflow's project or of the planning project (§1), so neither the user's choice nor a project's package is overridden, and a project run through `grida fx` without an `@grida/fx` of its own is hosted by the one `grida` carries. `python -m grida.fx` sets neither: Node then comes from `PATH`, and the host program only from the projects.

**The dialect** is Node's type stripping: erasable syntax only, imports that name the `.ts` file, `#` subpath imports in place of tsconfig `paths`, and `"type": "module"` in the project's `package.json`. A project type-checks with `@grida/fx` installed, `@types/node` as a dev dependency (the package's own `.d.ts` files name Node's types, such as `Buffer`), and a tsconfig that sets `allowImportingTsExtensions`, `erasableSyntaxOnly`, `verbatimModuleSyntax`, `noEmit` and `"module": "nodenext"`.

**Stand-ins.** `run(target, { standIn })` serves `standIn` in the caller's own process, with §5.7's session. Node has no `socketpair()`, so the SDK:
1. listens on a Unix-domain socket in a fresh private folder (`mkdtemp`);
2. connects to it and accepts the connection;
3. closes the listener and removes the folder;
4. gives the accepted end to the engine as its standard input, with `--stand-in -`.

`RunResult.standIn` says whether a run was a stand-in run. `grida-fx run --stand-in stand-in.ts#answer` serves the same function from a stand-in host.

| TypeScript | Protocol |
|---|---|
| `standIn(call)`, a function, or an async function, of one `StandInCall` | `stand_in.answer`; the SDK answers one call at a time, in the order the requests arrive |
| `call.capability`, `call.route.id`, `call.route.fingerprint`, `call.key`, `call.instance.id`/`.path`/`.step` | the params of the same names |
| `call.takes`, `call.take` | `take`, and its last number, as for `ctx.instance` |
| `call.request` | `request`, with every file value replaced by an `InputFile` built from `files` |
| `call.files` | `files`: a `ReadonlyMap` of `InputFile` objects by digest |
| `call.params` | the params as they arrived |
| `call.signal` | an `AbortSignal`, aborted by `$/cancel` for this call |
| `return { files: { name: bytes, an InputFile, {bytes, kind?} or a file URL }, data }` | an answer; a `Uint8Array` leaves `kind` out, an `InputFile` gives its kind, `{bytes, kind}` its `kind`, and a `file:` URL the suffix rule's |
| `Answer.json(value)`, `Answer.turn(text, toolCalls)`, `Answer.submit(arguments)` | `data` of `{"json": value}`, of an `agent.turn`, and of an `agent.turn` that calls `submit` |
| `return DECLINE` | `{"decline": true}`; returning `undefined` is a fault of the stand-in. `isDecline(value)` tells `DECLINE` (of any copy of `@grida/fx`) from an answer, for a stand-in that wraps another: comparing with `DECLINE` does not narrow the type |
| `throw new CallRefused(msg)`, `throw new CallFailed(msg)` | `capability_refused`, `call_failed` |
| anything else thrown | `internal`: the engine stops the run, and `run` rejects with that value once the engine has ended |

**Adapters.** `adapter(declaration)` declares a project adapter ([adapters.md](adapters.md) §9), with the brand `Symbol.for("grida.fx.adapter")`; a module exports it (`export const acme = adapter({…})`), and the host serves it as §5.8 says. The declaration is checked when it is made, with the Python SDK's sentences. Its types refuse what the check refuses where they can see it: a declaration without a capability, `requests` without `send`, `jobs` without `submit` and `collect`, a field `adapter()` does not know, and a function whose outcome its method does not take or holds a member that outcome does not have (`cost_usd` for `costUsd`, or a `refused` with a `costUsd`). Each function's `call.capability` is typed as the capabilities it serves, inferred through `const` type parameters.

| TypeScript | Protocol |
|---|---|
| `adapter({ requests: ["image.generate"], jobs: ["video.generate"], send, submit, collect, version: 1 })` | `adapter.load`'s `adapter`, as in Python; `version` is an integer of at least 0, or `null`, the default. `adapter()` refuses, with a `TypeError` where the module makes it, what Python's refuses, a declaration that is not an object, and a field it does not know (`an adapter declaration has no field 'versoin'`); the module then fails to import (§5.1). A member whose value is `undefined` is left out |
| `send(call)`, `submit(call)`, `collect(call)`, each a function or an async function of one `AdapterCall` | `adapter.send`, `adapter.submit`, `adapter.collect`, answered concurrently, in any order, on the host's event loop: a function that never yields holds the host |
| `call.capability`, `call.route` (`id`, `model`, `provider`, `contract`), `call.baseUrl`, `call.key`, `call.attempt`, `call.handle`, `call.takes`, `call.take` | the params; `call.handle` is `undefined` outside `collect`, whose call is typed `CollectCall` (`AdapterCall<C, JobHandle>`) |
| `call.request`, `call.files` | `request`, with every file value replaced by an `AdapterFile` built from `files`; `files` as a `ReadonlyMap` by digest, each with `digest`, `kind`, `size`, `path` and `bytes()` |
| `call.signal` | an `AbortSignal`, aborted by `$/cancel` for this call, with a `Cancelled` as its reason |
| `await call.fetch(url, { method, headers, json, body, contentType, lane, credential, timeoutS, maxResponseBytes })` | `fetch`, carrying the call's `call_id`: resolves to a `FetchResponse` (`status`; `headers`, `[name, value]` pairs in order; `body`, a `Uint8Array`; `json()`, `text()`), or rejects with a `FetchError` carrying the failure's `phase` and `reason`. `url` is a string or a `URL`; `headers` an object or `[name, value]` pairs (a `Headers`, a `Map`), its names lowercased; `json` any JSON value, `null` included; `body` a `Uint8Array` with its `contentType`. What the engine would refuse with `-32602`, an option `fetch` does not know, and `json` with `body` reject with a `TypeError` before anything is sent; an error answer rejects with the `EngineError` of its code (`Cancelled` for a call the engine dropped) |
| `return { outcome: "answered", files, data, costUsd, usage }`, `Answered.json(value, { costUsd, usage })` | `answered`; `files` and `data` may be left out (`{}` and `null`); its files as a stand-in's answer takes them, but an `AdapterFile`, with its kind, where a stand-in's takes an `InputFile`; `usage` `{ inputTokens, cachedInputTokens, outputTokens }` |
| `{ outcome: "not_received", reason, retryAfterS }`, `{ outcome: "refused", reason }`, `{ outcome: "failed", reason, retryable, costUsd }` | `not_received`, `refused`, `failed`; `retryable` is required |
| `{ outcome: "accepted", handle }`, `{ outcome: "uncertain", reason }` | `accepted`, `uncertain`; a handle is an object of JSON values |
| `{ outcome: "ended", reason, costUsd }`, `{ outcome: "unreachable", reason }` | `ended`, `unreachable` |
| `$/cancel` | answers the call `cancelled` at once and aborts `call.signal`; what the function returns or throws later is discarded |
| anything thrown, a value that is not an outcome its method takes, or an outcome the SDK cannot send | `internal`, `<name>: <message>`, its stack on stderr: the engine ends the call as the loss of its host would. A fault ends only its own call. Each outcome is held to its members when it is sent, as Python's are when made: a member it does not have (`Refused holds outcome and reason, not 'costUsd'`), a blank `reason`, a `costUsd` or a `retryAfterS` that is not a finite number of at least 0, a `usage` count that is not an integer of at least 0, a handle that is not an object; a member whose value is `undefined` is left out |

The types `SendOutcome`, `SubmitOutcome` and `CollectOutcome` say which outcomes each function may return; a function written on its own says its outcome type, since TypeScript widens an object literal's `outcome` to `string` otherwise. `new AdapterCall(params, { method, fetch, signal })` builds a call for a unit test, with a function that stands in for the engine's `fetch`.

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
    "data": {"json": {"caption": "A red square on a plain ground."}}
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
