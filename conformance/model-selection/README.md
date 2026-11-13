# Model and provider selection

Canonical provider-free examples of routing precedence. `in/` is the complete project;
`case.yaml` runs each variation and `expected/` records its resolved graph and identities.

| Intent | Selection | Result |
|---|---|---|
| Capability default | Canonical model + provider preference | Provider-native route |
| One step uses another model | Step `route` with bare model | Same provider preference |
| One step uses an exact provider | Qualified step `model@provider` | Exact route |
| Switch provider | Change preference | Resolve same model intent on that provider |
| Existing qualified default | Qualified project selector | Same identity as equivalent bare selection |
| Bare selector without preference | Model only | Actionable missing-preference refusal |

[Optional cost forecasts](../unknown-cost/) covers complete unpriced routes, estimates,
explicit caps and strict forecast coverage. [Route inheritance](../route-extends/) covers
`extends`; discovery's local-HTTP integration tests cover new GG/OpenRouter models,
metadata caching, offline operation and frozen prepared plans without real provider calls.
