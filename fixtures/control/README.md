# Cancellation harness

Three local steps: save a checkpoint, wait at a gate, then pass through the result.
Cancellation delays cooperative node cleanup for three seconds, making acceptance,
wait timeout and completion distinguishable. No providers or charges.

Copy this directory into a fresh folder for each test so the cache cannot hide
execution. In that folder:

```sh
grida-fx run workflow.yaml --run runs/test --max-usd 0 --no-view
```

In another terminal, after the `hold` step starts:

```sh
grida-fx inspect runs/test --control --json
grida-fx cancel runs/test --wait --timeout 1s --json
grida-fx cancel runs/test --wait --json
```

The first waiter times out without retracting cancellation. The next verifies local
completion. To resume the same plan and exercise the saved checkpoint:

```sh
touch .control-release
grida-fx run workflow.yaml --run runs/test --max-usd 0 --no-view
```

`tools/check_control.py` verifies this journey, guards, signals, exact targeting and
schema-valid results using the built engine.

From a source checkout, `python/.venv/bin/python tools/demo_control.py` prepares a
fresh project and prints the equivalent commands through the checkout SDK, including
a persistent viewer service. Rebuild the engine first after source changes.
