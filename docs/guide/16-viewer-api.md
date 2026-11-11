# Connect to the FX viewer

Use the viewer API to connect a chat, editor, or browser automation to an FX
viewer the user is already working in. You can read the current selection,
observe changes, and ask the viewer to show a recorded instance or workflow scope.

The viewer API requires FX 0.8.1 or later. An older installed viewer may not
expose `window.fx_viewer`, and an older `@grida/fx` package may not contain the
client entry shown below.

First open a plan or recorded run using [Viewing workflows and runs](07-viewing.md).
The viewer must have loaded its evidence before context is available. Your
application owns starting FX, opening the page, and activating its tab or window.
The API operates on that page's current content; it does not execute workflows,
change execution picks, or modify run records.

## Read the current selection

Browser automation can evaluate these calls inside the viewer page. They also
work in that page's browser console:

```js
const viewer = window.fx_viewer;
const context = viewer.getContext();
console.log(context.subject, context.scope_id, context.selection);

const unsubscribe = viewer.subscribe((next) => {
  console.log("Viewer context changed:", next.selection);
});

// Stop observing when this consumer is finished.
unsubscribe();
```

These page methods are synchronous. `subscribe` delivers an initial context and
then relevant changes. A read before evidence is loaded throws `not_ready`.

A plan context can look like this:

```json
{
  "subject": {
    "kind": "plan",
    "workflow": { "id": "icon", "source": "workflows/icon.yaml" }
  },
  "scope_id": null,
  "selection": {
    "kind": "instance",
    "instance_id": "prepare#1",
    "path": "prepare",
    "step": "prepare",
    "take": [1]
  },
  "sampled_at": 1794398400000
}
```

`subject.kind` is `plan`, `run`, or `catalog`. Workflow metadata and a durable
subject reference are included only when available. A project run/plan page can
identify its recorded service entry; a standalone page can omit that reference.
`scope_id: null` means the workflow root. `sampled_at` is the capture time in UTC
epoch milliseconds. `retained: true`, when present, means the viewer is showing
its last loaded data after a read failed.

Selection has several meanings:

| `selection.kind` | Recorded reference |
| --- | --- |
| `instance` | `instance_id`, with `path`, declared `step`, and `take` when recorded. |
| `scope` | `scope_id` for a selected imported workflow card. |
| `pending_repeat` | `path` for a repeat that has not expanded yet. |
| `scope_boundary` | `scope_id` and `side: input` or `output` for a workflow interface card. |

`selection: null` means nothing is selected. An inspected take is not necessarily
the take picked for execution. Use recorded identifiers from context or FX
evidence; do not construct identifiers from card labels or DOM attributes.

Context contains references, not the full graph, input/output values, diagnostics,
or media. Read those separately using the
[CLI or SDK run interfaces](09-sdk-runs.md).

## Connect from a host application

Install the JavaScript package in your application:

```sh
npm install @grida/fx
```

Import its browser-safe client entry. In this example, `viewerWindow` is an
existing browser window reference your application holds, and `viewerUrl` is
that viewer's URL. Wait for the viewer to load before connecting:

```js
import { ViewerClient } from "@grida/fx/viewer/client";

const client = new ViewerClient({
  targetWindow: viewerWindow,
  origin: new URL(viewerUrl).origin,
  timeoutMs: 5000,
});

const unsubscribe = await client.subscribe((context) => {
  console.log("Current selection:", context.selection);
});

const context = await client.getContext();
console.log(context.subject);

// Release this consumer's resources when it is finished.
unsubscribe();
client.dispose();
```

The client uses `postMessage` and validates replies from the supplied window and
exact origin. The origin includes the scheme, hostname, and port:
`http://127.0.0.1:8787` and `http://localhost:8787` are different origins.
Same-origin peers are accepted. A different-origin host needs the explicit grant
described below; opening the viewer alone does not authorize it.

Use one client per source/target window pair. Multiple local listeners can share
it; removing one listener leaves the others subscribed. Client operations are
asynchronous, and `subscribe` resolves to an unsubscribe function. The default
request timeout is 10 seconds when `timeoutMs` is omitted.

The client import contains no Node SDK, engine launcher, or viewer UI. Normal npm
installation can still install the matching optional native engine dependency.
The viewer application itself ships with the engine; this client does not render
a viewer component or start a service.

## Capture context when a user asks a question

Subscriptions help your UI follow the selection, but a previous notification may
be older than the user's current selection. Read a fresh snapshot when the user
submits their message:

```js
const context = await client.getContext();
const request = {
  text: "Why did this step fail?",
  viewer_context: context,
};
// Attach request.viewer_context to your application's user request.
```

The receiver can inspect evidence for the referenced instance and explain it.
Keep the captured snapshot with that request: later selection changes should not
change which instance the question referred to. The API does not send messages to
an agent or decide what the agent should do.

## Show the relevant instance or scope

Use `present` to request a presentation action. Supply the exact recorded instance
ID for the take you want to inspect, rather than a declared step name:

```js
const context = await client.present({
  action: "reveal_instance",
  instance_id: "prepare#1",
});
console.log(context.selection);
```

Replace `prepare#1` with an instance from your loaded plan or run. For automation
executing inside the viewer, use the same action with
`window.fx_viewer.present(...)`; its result is synchronous.

| Action | Arguments | Effect |
| --- | --- | --- |
| `reveal_instance` | `instance_id` | Enter its recorded containing workflow, select its card, and reveal the inspected take. |
| `open_scope` | `scope_id`, or `null` for root | Enter a recorded workflow scope and clear selection. |
| `clear_selection` | None | Clear selection while keeping the current scope and camera. |
| `fit` | None | Fit the current workflow while keeping selection. |

A successful call returns context after applying the presentation. It does not
wait for paint, animation, or media decoding, and does not activate the host's tab.
An unknown target returns `not_found` without navigating. The project catalog has
readable context but no workflow canvas, so presentation there is `unavailable`.
Artifact-specific preview controls are not exposed by this API.

Actions apply to whatever subject the page displays when it handles them. Check
context and discard outdated intent when the user navigates. Context updates do
not distinguish user actions from programmatic ones; avoid echoing every update
back into `present`.

## Authorize a different-origin host

Trusted preload code or browser automation executing **inside the viewer page**
can authorize an exact host window and HTTP(S) origin. For a popup whose known
host is its opener:

```js
const hostWindow = window.opener;
if (!hostWindow) throw new Error("This viewer has no opener window.");

const revoke = window.fx_viewer.allowHost(hostWindow, "https://editor.example");
```

Choose the expected host origin explicitly. Call `allowHost` once for each
window/origin pair and retain its revocation function. Call `revoke()` when the
integration is removed. The grant must be installed before the host uses the
client. A parent or opener is never automatically
trusted, and an ordinary cross-origin host cannot authorize itself through a
window message.

This grant only authorizes viewer messaging. The default local server still
refuses iframe framing and cross-origin HTTP access. Use a top-level window or a
desktop-managed page; any change to framing policy is a separate host concern.

## Handle failures and page navigation

The client rejects failed calls with `ViewerClientError`, whose `code` and
`message` describe the outcome. Page-local calls also expose a `code` for page
refusals; `timeout` and `disposed` are client-side errors. Common codes are:

| Code | What to do |
| --- | --- |
| `not_ready` | Wait until the page has loaded its evidence and, for presentation, mounted its canvas. |
| `not_found` | Check the recorded target against the currently displayed subject. |
| `unavailable` | Check that the page supports this operation and its evidence remains readable. |
| `invalid_request` | Correct the action or its arguments. |
| `unsupported_method` / `unsupported_version` | Use a client/viewer pair that supports the same page protocol. |
| `timeout` | No reply was confirmed. Check loading, window/origin, authorization, and viewer availability. |
| `disposed` | Use a new client; the previous consumer has released its resources. |

A timeout does not prove a presentation action was unapplied. The client does
not automatically retry it. Use a fresh context read to inspect the current state
before deciding whether another action is appropriate.

After viewer navigation or reload, dispose the old client and create and
subscribe a new one once the new page is ready. Reapply a cross-origin grant in
the new viewer page when needed. Navigation clears page subscriptions and grants;
there is no automatic reconnect. Disposing a client does not close the page or
affect the FX service or workflow execution.

For the raw message format, context types, and compatibility rules, see the
[viewer page contract](../../spec/viewer.md).
