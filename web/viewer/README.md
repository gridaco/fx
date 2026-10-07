# FX viewer frontend

The Vite app reads `/api/view`: an `fx-graph-v1` plan or an `fx-viewer-run-v1`
recorded run. `@grida/fx-web` validates both boundaries and owns the vanilla
TypeScript controllers for requests, selection, artifact previews, and the SVG
canvas. Dagre lays out the recorded wiring. React mounts the controllers and
renders the shell and inspectors; workflow and viewport state do not live in
React hooks. Neither browser package imports the Node SDK or executes workflows.

The canvas supports selection, pan, zoom, and fit. Two-finger/wheel scrolling pans
both axes; trackpad pinch or Ctrl+wheel zooms around the pointer. Steps also appear in a
keyboard-accessible list. Nodes and connections cannot be edited. Plans retain
their planned states, estimates, unresolved values, and pending repeats; run mode
retains recorded spend, outputs, and manual Refresh.

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

For frontend development, first start a built engine on the proxy's fixed port:

```sh
grida-fx view --run /path/to/recorded/run --port 8080 --no-open
```

Then, from the FX repository root:

```sh
bun --no-env-file run dev:viewer
```

The Vite server proxies `/api` to `http://127.0.0.1:8080`. Open the local Vite URL
printed by that command. The built, embedded viewer does not need a dev server.
