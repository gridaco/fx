# FX viewer frontend

The Vite app reads `/api/view`: an `fx-service-index-v1` project catalog, an
`fx-graph-v1` plan, or an `fx-viewer-run-v1` recorded run. Project pages select
scoped APIs under `/p/PROJECT_ID/runs/RUN_ID/` or `/p/PROJECT_ID/plans/PLAN_ID/`.
`@grida/fx-web` validates these boundaries and owns the vanilla
TypeScript controllers for requests, selection, artifact previews, and the SVG
canvas, which places cards in the cells the engine serves (`api/layout`, or a run's
snapshot). React mounts the controllers and
renders the bar and the details panel that float over the canvas; workflow and
viewport state do not live in React hooks. Neither browser package imports the Node SDK or executes workflows.

`ServiceIndexProjection` groups exact recorded workflow ID/source metadata and
selects the newest-created run, including failures. Native expandable history
contains named runs and saved plans; older unfinished evidence stays visible.
Creation metadata never participates in planning or caching. Legacy entries
without a recorded source remain separate.

The canvas supports selection, pan, zoom, and fit. Two-finger/wheel scrolling pans
both axes; trackpad pinch or Ctrl+wheel zooms around the pointer. Cards take keyboard
focus, and a corner button opens the steps as a list. Nodes and connections cannot
be edited. Plans retain their planned states, estimates, unresolved values, and
pending repeats; run mode retains recorded spend, outputs and live observation.

From the FX repository root:

```sh
bun --no-env-file install --frozen-lockfile
bun --no-env-file run check:viewer
bun --no-env-file run test:viewer
bun --no-env-file run build:viewer
```

The production build writes `web/viewer/dist`. Rust embeds that directory; it is
generated and ignored. Vite does not read env files, and the frontend ships no
external font or asset dependencies.

The browser regression check uses the production bundle with synthetic plan/run
responses and the browser client. Install Chromium once, then run it after building:

```sh
bun --no-env-file x playwright-core install chromium
bun --no-env-file run test:viewer:browser
```

`FX_CHROMIUM_EXECUTABLE=/path/to/chromium` can select an existing browser. The check
opens temporary loopback fixtures, makes no provider calls, and needs no running FX
service. It verifies nested takes, narrow-pane panel occlusion, expanded-deck
reveal after panning, selection/camera preservation, independent viewer windows,
reload, and origin authorization. CI runs it with Chromium, including real canvas
geometry and normal motion; the unit suite separately tests the viewport math.

For frontend development, first start a built engine on the proxy's fixed port:

```sh
grida-fx start --project /path/to/project --port 8080
```

Then, from the FX repository root:

```sh
bun --no-env-file run dev:viewer
```

The Vite server proxies root and scoped entry APIs to `http://127.0.0.1:8080`.
Open the local Vite URL printed by that command; entry pages keep that origin.
For standalone fixture development, the compatibility command
`grida-fx view --run /path/to/run --port 8080` also works.
The built, embedded viewer does not need a dev server.

## Page API

For application and automation usage, start with
[Connect to the FX viewer](../../docs/guide/16-viewer-api.md).

The engine-shipped page exposes `window.fx_viewer`. Browser automation evaluates
these calls in that viewer's realm, without inspecting the DOM:

```js
const context = window.fx_viewer.getContext();
const unsubscribe = window.fx_viewer.subscribe((next) => console.log(next.selection));
window.fx_viewer.present({ action: "reveal_instance", instance_id: "prepare#1" });
window.fx_viewer.present({ action: "open_scope", scope_id: null });
unsubscribe();
```

These methods are synchronous. `getContext()` captures the currently loaded
evidence and presentation; `present()` returns context after applying its action.
Wait until evidence is loaded (`not_ready` otherwise). `fit` and `clear_selection`
are also supported. The project catalog has readable context but no graph
presentation. Missing instances/scopes return `not_found` without navigation.

An application with an existing viewer window can instead use the asynchronous
[`@grida/fx/viewer/client` SDK](../../js/fx/README.md#browser-viewer-client).
The page uses the same repo-shared contract for both paths. Compact contexts use
recorded identifiers; a selected take is inspection, never an execution pick.
Capture context afresh when sending a user message; subscriptions are latest-state
notifications and may be coalesced. Hosts own tab activation.

Same-origin peers are accepted. For a cross-origin host, trusted preload or browser
automation code in the viewer realm must first call:

```js
const revoke = window.fx_viewer.allowHost(hostWindow, "https://host.example");
// Revoke before discarding the host integration.
revoke();
```

Use the actual host window and exact HTTP(S) origin; a parent/opener is never
automatically trusted. This grant cannot be requested through `postMessage`.
It grants only context/presentation messaging. Loopback HTTP, artifact confinement,
CORS and `frame-ancestors` policies stay unchanged. A browser popup/window or a
desktop-managed page does not require cross-origin framing. Grants and subscriptions
end with page navigation; hosts recreate their client afterward. Static showcase
canvases do not install a global page endpoint.

See [spec/viewer.md](../../spec/viewer.md) for the normative wire contract and
[viewer-page.test.ts](../fx-web/tests/viewer-page.test.ts) for provider-free
presentation, peer, subscription and client/page integration checks.
