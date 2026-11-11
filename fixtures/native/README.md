# Managed native operations

Provider-free projects proving the final adapter extension boundary. `python/` and
`typescript/` implement the same synthetic provider protocol. Neither opens a network
connection, even in a live invocation. The source text is a committed literal; no
external media or generated artwork is used.

A project node calls `acme.native`, which is absent from FX's standard capability
schemas. It passes a file, a provider operation name, an unknown integer field and
an unknown nested object. The adapter reads the file, records one synthetic submit
in `.native-submits` and returns a task handle. Collection waits for the local
`.native-release` gate. The journal is fixture state, not a provider integration or
part of portable FX records.

The `reject` mode returns a deterministic provider failure; `uncertain` returns an
ambiguous submission. Both carry structured provider diagnostics with deliberately
made-up secret, URL, path and input echoes. The engine must redact them while
retaining the error code, field and reconciliation ID. No real key is needed.

## Try it

Copy either project into a fresh directory. Python needs the `grida` package;
TypeScript needs Node 24+ and `@grida/fx` installed in its dependency tree. In that
directory, set `ACME_NATIVE_KEY` to any made-up string and `GRIDA_FX_NETWORK=off`.
Then:

```sh
grida fx plan native.yaml --inputs inputs.yaml
grida fx run native.yaml --inputs inputs.yaml --run runs/demo --live --max-usd 1
```

While collection waits, another terminal in the same directory can cancel:

```sh
grida fx cancel runs/demo --wait
touch .native-release
grida fx run native.yaml --inputs inputs.yaml --run runs/demo --live --max-usd 1
```

The journal has one line after resume: collection did not resubmit. Repeating the
workflow offline replays its result. The synthetic adapter reports $0.01, but a run
cancelled while the task is outstanding retains its $0.05 hold; later collection
adds no charge. These are accounting fixtures, not provider prices.

## Automated evidence

- `python/tests/test_native_engine.py` and `js/fx/test/native-engine.test.ts` copy
  these projects and run the actual engine and SDK node/adapter hosts with the
  network off. They use the public cancellation API and resume the same run.
- They distinguish result-cache reuse from call-cache replay by changing the node
  version while preserving the request. Changing the unknown field, source bytes
  or operation name produces a different call key and exactly one new submission.
- They compare SDK, CLI inspection and observation diagnostics, verify redaction,
  and rerun an uncertain call without submitting it a second time. Both SDKs also inject
  file echoes into every optional diagnostic metadata field and verify that diagnostic-enabled
  exports, errors and uncertain-job records withhold them.
- `crates/grida-fx-runtime/tests/calls.rs` exercises the real Tripo adapter over
  synthetic HTTP exchanges through the engine's store, ledger and retry owner:
  single-image success, offline replay, provider rejection and uncertain receipt.

The [provider boundary](../../docs/wg/provider-boundary.md) records ownership;
[adapters.md](../../spec/adapters.md) is the execution contract. A real adapter must
implement its provider's wire, upload and job semantics and classify outcomes
honestly; arbitrary JSON does not supply those semantics automatically.
