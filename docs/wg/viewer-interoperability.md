# Viewer interoperability

**Status: v1 accepted and implemented in source for
[#126](https://github.com/gridaco/fx/issues/126), 2026-11-11; pending release.**
The delivery is the page-level API, `postMessage` transport, context subscriptions,
presentation actions, a repo-shared contract, and the browser-safe
`@grida/fx/viewer/client` entry. [The viewer contract](https://github.com/gridaco/fx/blob/main/spec/viewer.md) is
normative; [browser/client guidance](https://github.com/gridaco/fx/blob/main/js/fx/README.md#browser-viewer-client)
describes the implemented interface.

## Model

An FX viewer is a page. Its host or browser automation already has a reference to
that page's window and can communicate with it through `postMessage`.

The page exposes three capabilities:

- read its current inspection context;
- subscribe to context changes;
- request presentation actions.

The window reference addresses the viewer. FX needs no additional viewer ID,
subject-generation token, or connection manager. The host owns its tabs and their
activation; FX owns the meaning of context and navigation inside the page.

## Distribution

Ship an embedder client SDK with this interface. Consumers should not have to
implement the message contract and keep it synchronized by hand.

| Piece | Ownership and distribution |
| --- | --- |
| Shared contract | Repo-shared message types and runtime validation, used by both client and viewer builds, with no Node or UI runtime dependencies. It can remain an internal module. The wire format is documented in `spec/`. |
| Client SDK | A public, browser-safe `@grida/fx/viewer/client` entry in the existing npm package. It provides typed operations and subscriptions for an existing viewer window. |
| Viewer application | The page implementation, built into and shipped with the native FX engine. It is not published as a UI/component SDK for this first version. |

Keep the small client in the existing `@grida/fx` release for v1. Use an explicit
subpath so browser consumers do not load the Node-oriented root SDK:

| Entry | Role | Delivery |
| --- | --- | --- |
| `@grida/fx/viewer/client` | Communicate with a viewer already running in a page/window. | Public SDK in this first delivery. |
| `@grida/fx/viewer` | A possible future SDK for rendering/mounting the viewer itself. | Future option; do not publish an empty entry now. |

Rendering a viewer locally and controlling one across a window boundary remain
different jobs. A future viewer SDK could instead become a separate package if
its UI dependencies or release needs justify that. The published client subpath
would remain supported; it need not be renamed to accommodate a viewer SDK.

The client entry and its public declarations must resolve directly to browser-safe
modules, without importing the package root, Node APIs, engine discovery/launching,
or viewer UI code. Tree shaking of a Node-oriented root is not the isolation
mechanism. Verify this boundary for both runtime imports and TypeScript types,
including a browser TypeScript consumer without Node ambient typings.

There is an installation-size tradeoff. The current `@grida/fx` manifest declares
platform binaries as optional dependencies, and normal installation can install
the matching engine package even when a consumer uses only the browser subpath.
Keeping binaries in separate packages does not make their installation conditional
on which entry is imported. Those binary files do not belong in the browser
bundle. Accept this existing package-install cost for v1; reconsider a separate
client package if a lightweight browser-only installation becomes a concrete need.

The engine continues to ship the standalone viewer application. A future viewer
SDK should reuse that viewer's rendering and presentation behavior, not introduce
a separate implementation. The client entry has no dependency on that future
viewer SDK or its UI dependencies.

The SDK accepts the target window and expected origin. It handles message
listeners, request/reply correlation, validation, typed results/errors, context
subscriptions, and ordinary listener cleanup. Request waits are bounded and
reported failures do not imply that a lost reply's action was never applied.
It does not silently retry presentation actions.

Use one client per target window/origin within a host and share it between local
listeners. Those listeners share the one wire subscription; removing one listener
does not unsubscribe the others. Client cleanup releases listeners/timers, settles
pending calls, and sends unsubscribe where possible. It does not close the page
or affect workflow/process lifetime.

Expose the relevant public types through the client entry. Include the
shared-contract dependency in the published package build; consumers must not
need to resolve a private workspace module. The client module graph includes no
viewer UI, engine binary, Node SDK, or React dependency. Constructing/using a
client does not install/start FX or create/manage host tabs.

For this delivery the viewer ships as an application, and its message API is a
public compatibility boundary. SDK package versions and engine versions do not
establish compatibility by themselves: use the message version and report
unsupported versions clearly.
The client follows the existing package/engine release for v1; include examples
against that released pair. No additional npm package or independently published
contract package is required.

## Messages

Use a small message envelope with a namespace, version, and request/reply
correlation. Request shape:

```json
{
  "channel": "fx_viewer",
  "version": 1,
  "type": "request",
  "request_id": "request-1",
  "method": "get_context",
  "params": {}
}
```

The viewer replies to the requesting window with the same `request_id` and either
a `result` or an `error`. Context updates use `type: context_changed` and carry
the latest context. Request IDs match replies to calls; they do not identify a
viewer, document, or run.

| Method | Behavior |
| --- | --- |
| `get_context` | Return a snapshot of the page's current inspection context. |
| `subscribe` | Register the requesting window for context changes and return the initial context. |
| `unsubscribe` | Stop sending context updates to that window. |
| `present` | Apply one presentation action to the currently displayed document. |

One context subscription per accepted window/origin is enough for v1. Repeating
`subscribe` reuses it; repeating `unsubscribe` is harmless. No subscription ID is
needed. The client SDK manages local callback listeners over that subscription.
The same-realm `window.fx_viewer` JavaScript facade exposes these semantics for
browser automation without a message round trip.

## Context

A context snapshot contains:

- `subject`: the displayed plan, run, or catalog, with its existing FX reference
  and workflow metadata when available;
- `scope_id`: the recorded workflow scope being inspected, or `null` for root;
- `selection`: the selected entity's kind and existing FX reference, or `null`;
- `sampled_at`: when the page captured this snapshot.

Use recorded identifiers. An instance, a workflow scope, a pending repeat, and a
workflow boundary are different kinds of selection. Recorded instance/take
metadata can explain a selection; renderer card IDs and deck indexes are not
public identifiers. Preserve the current single-selection behavior.

Existing service-entry references or an export's verified manifest digest may
identify the displayed subject. If a standalone source has no browser-visible
durable reference, leave that reference absent. Do not invent one or expose a
private filesystem path.

Read context from the page's loaded data. Keep snapshots compact: the full graph,
event stream, values, diagnostics, and media remain available through their
existing interfaces. An export exposes only its disclosed projection. If the
page is showing retained data after a read failure, report that condition.

Subscriptions describe current state, not a replayable event log. Updates may be
coalesced. They are informational and do not themselves request agent actions.
For message-time selection, the host requests a fresh snapshot when the user
submits; a previously received notification may be older.

## Presentation

The v1 `present` actions:

| Action | Arguments | Behavior |
| --- | --- | --- |
| `reveal_instance` | Recorded `instance_id`. | Navigate to its containing workflow, select its card, and reveal it. |
| `open_scope` | Recorded workflow `scope_id`, or `null`. | Open that workflow scope and clear selection. |
| `clear_selection` | None. | Clear selection in the current scope. |
| `fit` | None. | Fit the current workflow in the viewer. |

Resolve nesting from recorded scope membership. Do not infer it from labels or
path fragments. Reject an unknown target before changing navigation. Revealing
an earlier take foregrounds that card for inspection; it does not change the
workflow's picked output or canonical deck order.

A successful reply means the requested viewer state was applied. It does not
mean the host activated the tab, an animation finished, or media decoded. Replies
should clearly distinguish malformed requests, unavailable/loading content,
unknown targets, and unsupported actions. Avoid silent success for an action the
page could not perform.

Actions apply to the document displayed when the page handles the request. This
API does not provide optimistic locking or guarantee that delayed commands remain
applicable across navigation. A caller that needs a particular record must check
the current context and discard outdated intent when navigation changes it.

## Browser boundaries

Post to the intended window using an exact target origin. Accept requests and
send context only according to the host integration's expected source-window and
origin policy. A parent/opener reference by itself does not authorize access.
Replies and subscription updates go to that accepted peer, not a broadcast.
The SDK checks both the source window and origin of incoming replies/updates;
the viewer independently checks its requesting peer. Shared message validation
does not replace either side's peer policy.

The page accepts same-origin window peers. Cross-origin messaging requires trusted
code already executing in the viewer realm to grant an exact host window and
HTTP(S) origin; the grant returns a revocation function. Parent/opener identity
alone grants nothing, and no message method grants access. This keeps the default
local server's same-origin and framing boundary intact. A `MessagePort` transport
is not part of this delivery.

Page navigation, reload, and closure follow normal browser behavior. Callers
dispose their old client and create/subscribe a new one as needed. There is no FX reconnect,
process-restoration, or subscription-replay guarantee.

Keep the existing standalone host's HTTP and framing policies intact. Supporting
an additional framing origin, if needed, is a separate host-policy decision.
Presentation messages cannot execute arbitrary code, read arbitrary files, open
another source, or start/control workflows.

## Scope and verification

The first delivery includes the page-level interface, repo-shared contract, and
public client SDK subpath. Compositable viewer UI/components and artifact-specific
preview controls can be considered separately. No Desktop dependency, new project type,
launch configuration, or always-running service is introduced.

Provider-free verification should cover:

- two pages displaying the same run, addressed through their respective windows;
- context reads, subscription updates, and unsubscribe;
- exact instance reveal across recorded nested scopes;
- unavailable content and invalid targets;
- inspected takes remaining distinct from execution picks;
- expected versus unexpected message sources/origins;
- the published client subpath against the engine-shipped viewer, including local
  callback cleanup, browser bundling and type resolution without Node imports,
  and a clean npm install without private workspace dependencies;
- explicit behavior for an incompatible client/viewer message version;
- normal standalone use without a host.

The normative contract and client/browser examples are implemented in source.
Page/presentation and SDK tests hold context, authorization, subscriptions,
refusals, browser bundling and declaration isolation to the public contract.
Provider-free browser verification exercised independent viewers, nested takes,
selection callbacks, reload and explicit cross-origin grants. Static showcase
canvases do not install a global page endpoint. No dedicated artifact control,
provenance labels for user versus caller changes, or reconnect manager is added;
hosts should avoid echoing unchanged context into presentation calls.

## References

- [Issue #126](https://github.com/gridaco/fx/issues/126)
- [Local service](https://github.com/gridaco/fx/blob/main/spec/service.md)
- [Run observation](https://github.com/gridaco/fx/blob/main/spec/observation.md)
- [Portable run exports](https://github.com/gridaco/fx/blob/main/spec/exports.md)
- [Canvas layout](https://github.com/gridaco/fx/blob/main/spec/layout.md)
