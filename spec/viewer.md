# Viewer page API

Protocol v1 exposes inspection context and presentation in an existing viewer
page. It does not execute workflows, mutate records or picks, launch processes,
activate windows, or navigate to arbitrary sources.

## Transport and peers

Window messages use `channel: "fx_viewer"`, `version: 1`, and `type` equal to
`request`, `response`, or `context_changed`. Requests carry a nonempty string
`request_id`, `method`, and object `params`. Responses echo the ID and carry
either `ok: true, result` or `ok: false, error: {code, message}`.

The page accepts same-origin window peers. A cross-origin peer requires an
explicit grant from code already authorized to execute in the viewer's realm;
that grant binds an exact window and HTTP(S) origin. Parent/opener identity alone
does not grant access. Check source and origin before parsing or replying. Use
an exact target origin for every reply/update. HTTP/CORS/framing policies do not
change. No origin grant is accepted through the message protocol itself.

The SDK accepts only responses/updates from its supplied target window and exact
HTTP(S) origin. It never discovers, installs, or starts an engine.

## Context

A context is a JSON object with `subject`, `scope_id`, `selection`, and
`sampled_at` (UTC epoch milliseconds). An optional `retained: true` means a
previous projection remains visible after a failed read.

`subject` has `kind: plan`, `run`, or `catalog`. Optional `workflow` contains a
recorded `id` and portable `source` when available. Optional `reference` is either:

- `kind: service_entry`, `project_id`, `entry_kind: plan|run`, `entry_id`;
- `kind: export`, `manifest_sha256`.

Unknown/withheld references remain absent. No absolute local path, authorization
material, graph, input/output values, diagnostic body, or media bytes is exposed.

`scope_id` is a recorded navigable workflow scope or `null` for root.
`selection` is `null` or one of:

- `kind: instance`, `instance_id`, optional recorded `path`, `step`, `take`;
- `kind: scope`, `scope_id`;
- `kind: pending_repeat`, `path`;
- `kind: scope_boundary`, `scope_id`, `side: input|output`.

Selections use existing evidence references, not renderer card IDs. A take in
context identifies inspection, not the workflow's execution pick.

## Requests

| Method | Params | Result |
| --- | --- | --- |
| `get_context` | `{}` | Current context. |
| `subscribe` | `{}` | Current context; register one subscription for the exact source window/origin. |
| `unsubscribe` | `{}` | `null`; remove that peer's subscription. |
| `present` | An action below. | Context after the presentation was applied. |

Actions:

- `action: reveal_instance`, `instance_id`: enter its recorded containing workflow,
  select and reveal its card, including foregrounding a take inside a deck.
- `action: open_scope`, `scope_id: string|null`: enter a recorded navigable workflow
  scope or root and clear selection.
- `action: clear_selection`: clear selection, preserving scope/camera.
- `action: fit`: fit the current graph, preserving selection.

Resolve ancestry through recorded membership. Refuse unknown targets before
changing navigation. Successful presentation does not assert paint completion,
media decoding, or host activation. Requests apply to the currently displayed
subject; there are no viewer IDs, epochs, optimistic locks, or cross-navigation
delivery guarantees.

No loaded evidence returns `not_ready`. Catalog context is readable but graph
presentation returns `unavailable`. Other expected errors are `invalid_request`,
`unsupported_version`, `unsupported_method`, `not_found`, and `unavailable`.
Return safe messages without private state. Ignore unrelated message channels.

Updates carry `type: context_changed, context`. Send latest-state changes, not
execution history or raw pointer events; coalescing is permitted. Repeating
subscribe reuses the peer subscription; unsubscribe is idempotent. Navigation
and closure use normal browser behavior. Hosts dispose the old client and create
and subscribe a new one after page navigation/reload. No automatic reconnect is
promised. Updates do not label a change as user- or caller-initiated; hosts should
compare relevant context fields rather than echo every update into presentation.

## Same-realm JavaScript

The engine-shipped page exposes `window.fx_viewer` for browser automation and
trusted host code executing in that page's realm:

- `getContext()` returns current context synchronously.
- `present(action)` applies the same validated presentation and returns context.
- `subscribe(listener)` delivers initial context, then changes, and returns a
  local unsubscribe function. A listener's exception does not interrupt others.
- `allowHost(window, origin)` grants an exact cross-origin window peer and returns
  a revocation function. Call once per peer/origin and retain that function.

Unavailable evidence/presentation throws an error with `code` and a safe message.
The facade has no process lifecycle role. Grants and subscriptions belong to the
current page and do not survive navigation. The cross-origin grant does not
enable HTTP/CORS access or framing; a desktop host can install it through trusted
preload or ordinary browser automation. An ordinary cross-origin opener cannot
authorize itself. The page API does not expose arbitrary JavaScript evaluation.

## Browser client

`@grida/fx/viewer/client` exports a browser-safe `ViewerClient`, useful public
types, and typed errors. It has no runtime or declaration dependency on the Node
SDK root, launcher, engine lookup, UI implementation, or Node ambient types.

The client supplies `getContext()`, `present(action)`, `subscribe(listener)`, and
`dispose()`. Use one client per source/target window pair. Multiple local listeners
share one wire subscription. Removing one
listener does not unsubscribe the others. Disposal releases local listeners and
timers, settles pending calls, and attempts unsubscribe; it does not close the
viewer or affect execution.

Request waits are bounded. Timeout means no confirmed reply, not proof that a
presentation was never applied. Never automatically retry presentation. Validate
received messages. Incompatible responses and unavailable viewers produce clear
client errors. Package versions do not replace wire-version compatibility.

The types and validators are shared internally with the viewer build and included
in the published package. Installing `@grida/fx` can still install its optional
platform engine dependency; importing this subpath must not include it in a
browser bundle.
