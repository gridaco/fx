# One-shot capability calls

`grida-fx call` invokes one standard capability from the command line. It
validates and routes the request, admits it against a budget, manages caching,
retries and cancellation, and records the run through the execution path of a
one-step workflow. The command takes its request from arguments and starts no
viewer. The first supported capability is text-to-image `image.generate`.

## 1. Arguments and planning

```text
grida-fx call image.generate --prompt TEXT [--route SELECTOR]
  [--background auto|opaque|transparent] [--size auto|WIDTHxHEIGHT]
  [--live | --stand-in FILE#FUNCTION] [--max-usd USD]
  [--out FILE] [--dry-run] [--fresh] [--json]
```

`--prompt` supplies literal text, including expression markers and path-like
strings. Background and size use the defaults and validation of
[`image.generate`](capabilities.md#2-imagegenerate). Pixel dimensions are
positive decimal integers of at most six digits, with no leading zeroes.
This command accepts text input and produces one PNG called `image`.

FX finds the nearest `fx.yaml` at or above the working directory, as for other
project commands. It reads that project's route defaults, provider preferences,
route tables, provider declarations, budget and storage settings. An explicit
`--route` overrides the capability's default and follows the normal
[model selection rules](model-selection.md). When neither supplies a selector,
the command refuses and names the ways to select a route. A qualified selector
can select a built-in route outside a project. There FX uses its ordinary
projectless storage defaults in the working directory.

`--dry-run` validates the request, resolves the route and reports its forecast
and applicable budget. It creates no run, produces no artifact and dispatches
no generation. Provider-side validation and readiness apply at execution.
Existing model metadata preparation rules apply, including the
bounded read-only lookup for a selected missing image binding on a supported
provider and the network switch. `--dry-run` conflicts with `--live`,
`--stand-in`, `--fresh` and `--out`.

## 2. Admission, cache and records

`--live` admits provider dispatch. A live dollar-billed route requires a
ceiling from `--max-usd` or the project's `budget.max_usd`, in that order.
A route billed to the user's plan, including Codex and Grok
Build, is eligible for the existing zero-dollar admission path. The same
invocation uses the selected route for every attempt.

`--stand-in FILE#FUNCTION` answers the call offline in the existing stand-in
store and conflicts with `--live`. Ordinary invocations may use a compatible
result or call cache entry, including entries shared with an equivalent
one-step workflow. A cache hit can succeed without `--live`. Each invocation
that runs creates a new run record. Its workflow ID is `fx-call-image-generate`
and its `workflow.file` is the virtual source `fx:call/image.generate`. The
source has no editable file, takes file, or workflow repeat command. If an
authored run shares the ID, workflow-ID and named lookups use the authored
runs. The command reports its exact run folder for inspecting the inline call.

`--fresh` requires `--live` or `--stand-in`. It gives that invocation a fresh
cache identity so it can reach the selected adapter or stand-in even after a
previous success. It preserves the ordinary cache entry and previous run
records, and uses the same admission, attempt and cancellation rules.
For a long-job adapter, a matching ordinary call can collect a job recorded
under its call key. Each `--fresh` invocation requests new work under a
different key. Pending or uncertain jobs remain visible in `jobs`.

## 3. Delivery and results

An invocation that produces an image places it at `outputs/image.png` in its
run folder. With no `--out`, the command reports the path to this saved
artifact. `--out` additionally delivers a copy to the named file, resolving
relative paths from the working directory. Its parent directory must already
exist and allow a new file. FX refuses directory-form paths and occupied
destinations before running. It checks that the destination's parent still
resolves to the same path before publishing the copy without replacing an
entry created concurrently. If delivery fails, the run and its saved artifact
remain available.

## 4. Command result

With `--json`, a syntactically accepted invocation that reaches its command
result prints a single
[`fx-call-result-v1`](schemas/fx-call-result-v1.schema.json) document on
stdout. Progress and control messages go to stderr. Command-line syntax
errors use stderr and exit 2. The document always includes:

| Member | Meaning |
|---|---|
| `kind` | `fx-call-result-v1` |
| `status` | `planned`, `ok`, `refused`, `failed`, `cancelled` or `error` |
| `capability`, `route` | The capability (`image.generate`) and its resolved concrete route, or `null` for an unresolved route |
| `cache` | `step` for a result cache hit, `call` for a call cache hit, `miss` for uncached execution, or `null` when no completed cache evidence is available |
| `run`, `invocation_id` | The run folder and invocation ID, each `null` when unavailable |
| `artifact`, `output` | The saved run artifact and the final output path, each `null` when unavailable. Without `--out`, `output` is the same path as `artifact`. If delivery fails, the saved artifact path is still reported. |
| `charged_usd`, `ceiling_usd` | The booked amount and applicable ceiling in USD, each `null` when unavailable |
| `forecast` | `{low_usd, high_usd, unpriced_calls}` when planning produced a forecast, otherwise `null`. With unpriced calls, the overall bounds are `null` and `known_low_usd` and `known_high_usd` give the known subtotal. |
| `stand_in` | Whether the invocation uses a stand-in |
| `error` | `null` or `{code, message, diagnostic}`. `code` and the structured adapter `diagnostic` are `null` when unavailable. |

Run and artifact paths are relative to the invocation's working directory when
possible. A delivered output keeps the path the caller supplied. These paths
are local command results, outside portable record fields. A dry run has plan
facts and no run, invocation ID, artifact or output.

| Exit | Status | Meaning |
|---|---|---|
| `0` | `planned`, `ok` | Dry-run validation or execution and any requested delivery succeeded |
| `1` | `refused`, `failed` | Planning or admission refused the request, or execution or delivery failed |
| `2` | `error` | Argument, setup or fatal error |
| `130` | `cancelled` | Cancellation in the normal run lifecycle |

An interrupt before the runner takes ownership, or a forced exit, can end
the command before it prints a result. The run-control contract governs the
recorded invocation when a run already exists.
