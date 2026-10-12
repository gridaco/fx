# Distribution

How FX reaches its users, and what each way of running it may and may not change. FX is a Grida
product: its main command is `grida fx`, carried by the npm `grida` command. The engine is the
same binary however it is started. This contract changes no execution, identity, cache, spending,
run-record or observation semantics.

## 1. The ways to run FX

| Command | Installed by | What carries the engine | Who it is for |
|---|---|---|---|
| `grida fx <verb>` | npm `grida` ([gridaco/grida](https://github.com/gridaco/grida)) | `@grida/fx`, pinned exactly by `grida`, and its engine package for the machine | Everyone: the main command |
| `grida-fx <verb>` | npm `@grida/fx` | `@grida/fx`'s engine package for the machine | The JavaScript SDK, and FX on its own |
| `python -m grida.fx <verb>` | PyPI `grida` | The wheel itself | The Python SDK, and Python users without Node |

The dependencies run one way: `grida` → `@grida/fx` → one engine package (`@grida/fx-<os>-<cpu>…`,
an optional dependency npm installs by `os`, `cpu` and `libc`). FX never depends on `grida`.
The platforms are `@grida/fx`'s ([RELEASING.md](../RELEASING.md)). Where FX has no engine,
`grida` still installs and `grida fx` fails with `@grida/fx`'s own explanation.

SDK imports stay `@grida/fx` (JavaScript) and `grida.fx` (Python). `grida` re-exports no FX API.

## 2. The invoker

A launcher tells the engine which command started it through `GRIDA_FX_INVOKER`:

| Value | Command, as its user types it |
|---|---|
| unset, empty or `grida-fx` | `grida-fx` |
| `grida` | `grida fx` |
| `python` | `python -m grida.fx` |

Any other value is refused before the command line is read: `grida-fx: GRIDA_FX_INVOKER must be
one of grida-fx, grida, python`, exit status 2. Each launcher sets the variable for the engine it
starts, replacing any inherited value.

The invoker changes only the command layer's own words in terminal output:

- usage and help (`Usage: grida fx run [OPTIONS] <TARGET>`);
- the prefix of an error on stderr (`grida fx: <message>`), and a launcher's own messages;
- hints that name a command to type next (`next      grida fx run case`).

Everything else is the same under every invoker:

- **Documents.** What the core computes and prints or writes (graphs, plans, problems, warnings,
  prices, schemas) and everything written to a project, a run folder, the cache or an export
  (`plan.json`, takes files, `fx.lock`, records, events) name the engine `grida-fx`. A record made
  under `grida fx` is byte-identical to one made under `grida-fx`.
- **Identity.** `--version` prints `grida-fx <version>`; the node protocol's `initialize` and the
  provider `User-Agent` name `grida-fx`.
- **SDK calls.** The SDKs always run the engine as `grida-fx`, whose error prefix they read,
  even when their own process inherited another invoker (a node body run under `grida fx`).

## 3. The host contract

A host command is one that carries FX as a verb of its own; `grida` is the only one. `@grida/fx`
exports its launcher for hosts as `@grida/fx/cli`:

- `HOST_CONTRACT`: `1`, the version of this section.
- `launch(argv, { invoker })`: runs the engine with `argv` (the arguments after `fx`), this
  process's standard streams and working directory, and ends the process as the engine ended:
  its exit status, or the same signal. The engine runs in a process group of its own; SIGINT,
  SIGTERM, SIGHUP and SIGQUIT are passed on once per interruption; Ctrl-Z stops both. `invoker`
  is `grida-fx` (the default) or `grida`; any other throws a `TypeError` before anything runs.
- `INVOKER_VARIABLE` and `COMMANDS`: the variable of §2, and each invoker's command.

A host:

1. depends on `@grida/fx` at exactly one version, and is published after it;
2. imports `@grida/fx/cli` only when its first argument is `fx`, so no other verb loads FX;
3. refuses a `HOST_CONTRACT` it was not written for, telling the user to reinstall it;
4. calls `launch(rest, { invoker: "grida" })` and does nothing after it: the launcher owns the
   process from then on;
5. never finds, spawns or replaces the engine itself (`GRIDA_FX_BIN` stays the user's), and adds
   no FX options of its own.

The `fx` branch of the stand-in host, [fixtures/host/grida](../fixtures/host/grida/), is the
reference for these five rules; `grida`'s launcher carries the same lines.

## 4. Versions and releases

FX is versioned and released on its own ([RELEASING.md](../RELEASING.md)). A Grida release that
moves FX forward happens in this order:

1. FX tags `v<version>` and publishes `@grida/fx` and its engine packages. The tag's source
   archive also serves `init --template` ([service.md](service.md) §2.1): an engine of that
   version reads its templates from it, so the tag must hold every folder its
   `examples/templates.json` names.
2. `grida` pins `@grida/fx` to exactly that version, bumps its own version, and publishes.

`grida` owns telling its users that a newer `grida` is available; that covers `grida fx`,
because the pin moves with `grida`'s version. `@grida/fx` and `grida-fx` never check for updates.

Provider keys are FX's own today ([providers.md](providers.md) §3). Reading the provider
credentials `grida` manages, which `grida` alone writes, is tracked in
[gridaco/fx#45](https://github.com/gridaco/fx/issues/45).

## 5. Holding to this contract

- The conformance runner takes `--invoker`: under `grida` or `python`, the command's terminal
  text must name that command at a verb, and every document must be unchanged.
- `tools/check_host.py` packs `@grida/fx` and the machine's engine package, installs them beside
  the stand-in host, checks §2's usage, errors and dispatch, and runs the whole conformance suite
  and the viewer check through `grida fx`. CI runs it on every change.
- A release candidate is verified through `grida fx` as well as `grida-fx` and
  `python -m grida.fx`; publishing refuses a candidate without those checks.
- `tools/check_versions.py` holds the stand-in host's `@grida/fx` pin to the workspace version.
