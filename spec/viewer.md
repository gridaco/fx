# Viewer page API

Protocol v1 exposes inspection context and presentation in an existing viewer
page. It does not execute workflows, mutate records or picks, launch processes,
activate windows, or navigate to arbitrary sources.

## Transport and peers

Window messages use `channel: "fx_viewer"`, `version: 1`, and `type` equal to
`request`, `response`, `context_changed`, or the optional `external_file` event. Requests carry a nonempty string
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
| `register_external_file_handler` | `{handler_id}` | `null`; delegate external file activations to this exact peer. |
| `unregister_external_file_handler` | `{handler_id}` | `null`; release only this peer's matching registration. |
| `external_file_result` | `{handler_id, activation_id, ok}` | `null`; acknowledge one delegated activation. |

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
- `handleExternalFile(handler)` installs one external-file handler and returns a
  local detach function, using the shared behavior described below. It does not
  require window messaging.

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

The client supplies `getContext()`, `present(action)`, `subscribe(listener)`,
`handleExternalFile(handler)`, and `dispose()`. Use one client per source/target window pair. Multiple local listeners
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

## External file activation

External file activation means handling a particular file outside the viewer's
own presentation, including a document tab inside an embedding application.
It carries no Open/Reveal/OS command enum. The host owns that behavior. Existing
inline media previews, workflow navigation, execution and context subscriptions
remain independent.

The shared TypeScript rendering layer owns a transport-independent injected
handler. With no handler, served asset links retain their actual `href` and
ordinary browser behavior. A registered handler owns ordinary primary-button
and keyboard activation: cancel default navigation synchronously, capture the
file and current subject, then invoke the handler. Modifier-clicks and
middle-button activations retain native link behavior. A source label without a
served URL stays non-interactive unless a handler is available. This feature
does not introduce a source-serving endpoint.

The browser-safe portable reference is exactly `{subject, target}`. `subject`
uses the existing subject contract and cannot be `catalog`. `target` is one of:

- `kind: artifact`, `artifact: {digest, url, name, kind, size}`. `digest` is 64
  lowercase hexadecimal characters; `size` is a nonnegative safe integer. `url`
  is null, a root-relative served URL, or an encoded HTTP(S) URL without credentials,
  query or fragment. A URL addresses bytes, never a filesystem location. `name`
  is a display label, not an absolute filesystem path.
- `kind: source`, `source`, optional `symbol` and `recorded_digest`. `source` is
  a normalized relative POSIX label, with no empty, `.` or `..` segments,
  backslash, colon or absolute root. A declared-package label is interpreted
  through its explicit source binding, not by assuming the planning project
  root. `symbol`, when known, is a module/builder identifier. A recorded digest
  is 64 lowercase hexadecimal characters and is optional.

Neither target carries absolute paths or media bytes. Native discovery and
resolution are specified in [files.md](files.md). They require the host's
explicit local record/project binding. An artifact resolves an available copy
of the recorded bytes; source resolves the current editable file, which can
differ from the source used by the run. File activation does not promise a
particular alias, historical source recovery or a declaration line.

### Delegation across a window boundary

`ViewerClient.handleExternalFile(handler)` returns a promise for a detach
function. It sends `register_external_file_handler` with a nonempty unique
`handler_id`. A page supports one explicit owner across same-realm and remote
handlers. A different peer's registration returns `unavailable`; context access
or a context subscription never grants file ownership. Repeating the same peer,
origin and handler ID is idempotent. The same authorized peer may replace its
registration with a new ID, allowing reconnection. Old cleanup or replies cannot
release or complete the new registration. A same-realm handler must be detached
before a remote peer can register, and vice versa.

An activation is sent only to its owner:

```json
{
  "channel": "fx_viewer",
  "version": 1,
  "type": "external_file",
  "handler_id": "host-registration-1",
  "activation_id": 1,
  "reference": {
    "subject": {"kind": "run", "workflow": {"id": "acme"}},
    "target": {
      "kind": "source",
      "source": "nodes/render.py",
      "symbol": "render"
    }
  }
}
```

`activation_id` is a positive safe integer increasing within the registration.
The client validates the exact source window, origin, version, handler ID and
portable reference, and ignores repeated or older activation IDs. Events are
not coalesced or broadcast to context subscribers. Capture subject and target
together at activation; do not replace them with later context after an async
lookup or selection change.

The client invokes its handler once and sends `external_file_result` with the
matching IDs and `ok: true` when it resolves, or `ok: false` when it throws or
rejects. No host exception text or native result path is sent back to the page.
Results from another peer or an old registration cannot settle the activation.

The supplied viewer bounds activation waits at ten seconds and admits at most
64 pending activations. Failure, timeout, revocation or detachment is an
unconfirmed outcome; it must not automatically repeat the handler or replay
browser navigation. Show only safe failure text. When a served artifact URL is
available, an explicit browser link can remain a separate user choice. A late
completion cannot turn an expired activation into a confirmed one. Returning
from a handler does not prove host paint, OS activation or file lifetime.

Detachment and client disposal attempt token-bound unregistration. Page disposal
and explicit cross-origin grant revocation release its remote owner and pending
waits. Unregistration is idempotent. A lost registration acknowledgement means
ownership may have been installed; clean it up best-effort and report the error,
without automatically retrying registration. Navigation/reload has the existing
page-lifecycle rules; this is not an automatic reconnect service.

These additive methods remain protocol version 1. Older viewers explicitly
return `unsupported_method`; new pages without file behavior return
`unavailable`. Package versions alone do not imply support. A future composable
viewer can accept the same injected handler directly, without this message
adapter or a second file behavior implementation.
