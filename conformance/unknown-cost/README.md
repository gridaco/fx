# Optional cost forecasts

Canonical provider-free proof of model compatibility independent of pricing. The project
uses synthetic routes and never contacts a provider. Run through the conformance harness.

| Case | Authored facts | Ordinary plan | Strict plan |
|---|---|---|---|
| `unknown` | Complete route, no price | Unknown forecast | Missing forecast |
| `estimated` | $0.12 estimate | Included in known subtotal | Accepted |
| `allowance` | $1 explicit cap | Unknown forecast; cap remains a reservation | Missing forecast |
| `reset` | Extend priced route, `price: null` | Clears inherited forecast | Missing forecast |

The aggregate is $0.12 estimated plus three unpriced calls. It is never a $0.12 total.
`--require-estimates` refuses before execution. Runtime tests exercise budget reservations,
settlement, retries and concurrency independently with synthetic adapters.

See [model selection](../model-selection/) for provider preference and per-step selection;
CLI `model_discovery` tests cover provider metadata, offline caches and prepared snapshots.
