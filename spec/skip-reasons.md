# Recorded skip reasons

**Status: implemented in source, 2026-11-11.** This is additive presentation
evidence within `fx-run-events-v1`, `fx-viewer-run-v1` and `fx-run-export-v1`.
It changes no execution decision, step or call identity, cache entry, selected
take, output value or member order.

## 1. Purpose

`skipped` is an outcome category. Its cause can describe normal control flow,
unused regeneration capacity, a rejected candidate or blocked progress. The
engine records causes at its existing decision points so a viewer can explain
them without evaluating workflow expressions or requiring author-written text.
No model call generates an explanation.

## 2. Descriptor

An optional `skip_reason` has this shape:

```json
{"code":"earlier_take_accepted","sources":["draw#2"]}
```

`code` is a lower_snake_case string. `sources` is always present and contains
distinct related instance IDs in canonical source order; it can be empty. These
references identify evidence, not a new dependency. A descriptor contains no
free-form explanation, authored expression, evaluated value, file path or URL.

The initial codes are:

| Code | Engine decision |
|---|---|
| `condition_false` | The step's condition was not satisfied. |
| `earlier_take_accepted` | An earlier accepted take made further takes unnecessary. |
| `earlier_take_settled` | An earlier take settled the regeneration sequence for another recorded reason. Acceptance is not implied. |
| `upstream_failed` | A prerequisite failure blocked this step. |
| `upstream_absent` | A required upstream result was absent. |
| `rejected` | Rejection and the workflow's policy left this path unused. |
| `assertion_skip` | A runtime assertion's skip policy excluded this step. |
| `no_candidate` | Selection found no available candidate; the operation may have started. |

Sources may identify a preceding take, a failed prerequisite or another existing
instance involved in the decision. Their presence does not mean that the skipped
body ran. A viewer must not infer nonexecution, failure or acceptance from the
`skipped` state or a generic cause.

## 3. Recording and projection

`scopes_updated` adds an optional `skip_reasons` object keyed by known instance ID.
It is a complete snapshot of causes for instances the expansion has left absent
or unnecessary. An empty object replaces previous snapshot causes. An absent map
in an older snapshot provides no cause and cannot preserve a stale newer one.
The map does not add absent instances to `instances`, create cards, change
membership or reorder members.

New expansion listings may additionally preserve each instance's canonical
`path` alongside `id`, `step`, `take` and `key`. This is an instance address,
not a filesystem path. It lets resumed invocations retain known display
instances without parsing IDs; older listings remain valid without it.

`node_skipped` can carry its own `skip_reason`, including a blocked prerequisite
or a selection operation with no candidate. This terminal node evidence remains
independent of snapshot membership; an empty expansion snapshot does not clear
it. A later start of that node clears its cause. A new invocation clears prior
causes before applying its own evidence. Replay never feeds these descriptors
back into scheduling or cached results.

The viewer applies snapshot causes to retained skipped instances when the run is
terminal. Pending instances in an unfinished run have no final skip explanation.
A recorded `node_skipped` cause may be shown while other work is still running.
Causes are omitted from succeeded, failed, running and waiting nodes. Source IDs
must resolve to nodes in the projected run; a malformed descriptor or one with
dangling references supplies no explanation.

Consumers ignore unknown optional fields and unknown future cause codes within
v1. They must preserve the underlying execution state and use generic feedback
when they cannot explain a cause. The current native viewer projects known codes
only. Missing, unsupported or malformed causes do not invalidate older runs and
must not be reconstructed from diagnostic prose, node names or topology alone.

## 4. Presentation and portable records

The viewer renders plain-language explanations from the code and recorded source
metadata. For example, an unused third take may say that an earlier take was
accepted and offer navigation to it. Titles and take labels identify existing
nodes; a source ID is never interpreted as a URL. A fallback can say that the
reason was not recorded. It must not turn absence of evidence into a guessed
business explanation.

The initial implementation records supported node-judge regeneration, conditional
exclusion, dependency and rejection handling, assertion skips and empty selection
at their engine decision points. It does not promise an explanation for every
skipped instance. In particular, early termination of a regenerating group through
`until` may lack a recorded cause and uses the generic fallback. The vocabulary
can grow additively as further decision points preserve their evidence.

Portable exports retain recognized structural descriptors by default, including
when diagnostic text and values are withheld. The positive projection keeps only
the recognized code and source IDs; arbitrary payload members are omitted. Every
source reference must remain within the exported node inventory. This metadata
adds no artifact inclusion, source discovery or execution access.

Older recordings may retain only a status or diagnostic text. Updating the
viewer does not rewrite their events or invent a final reason. A fresh run can
record richer causes while reusing existing valid cached work.
