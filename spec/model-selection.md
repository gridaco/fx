# Model selection and provider preferences

Model selection is authoring configuration. The engine resolves it to an ordinary concrete
route before pricing, approval, identity and execution. No planning or running operation
discovers models over the network. The words MUST and MUST NOT are used as in RFC 2119.

## 1. Selectors and defaults

A step's `route` and a project's `routes.<capability>` accept either:

- a qualified `<native-model>@<provider>`: an exact route, split at the last `@`;
- an unqualified model identifier: a nonempty string containing neither whitespace nor `@`.

Each step may choose a different model. The project's capability selection is only a default.
For a node that calls exactly one distinct capability, its nonempty `route` overrides that
default. A node that calls several capabilities continues to use each capability's default;
a scalar route does not specify which capability to override. A body cannot select another
route for one capability call. These rules do not introduce dynamic routing or paid fallback.

The workflow home's `routes` are overlaid by the planning project's, by capability. Home means
the workflow's source project, not the user's operating-system home. A separate optional
`provider_preferences` map in `fx.yaml` names one provider per capability. It merges in the
same order, independently of model selection. There is no user-wide configuration scope.

Selection proceeds as follows:

1. Choose the step selector where applicable, otherwise the merged capability default.
2. A qualified selector resolves exactly as before; preferences cannot override it.
3. An unqualified selector uses that capability's merged provider preference. Missing
   preference is a refusal naming the setting or a qualified selector as corrections.
4. Resolve the selected model, provider and capability in the admitted route catalog (§2).
   Missing or ambiguous bindings are refusals. Credentials do not choose a provider, and a
   missing key does not cause another provider to be selected.
5. Apply the existing route's feature, input-count, contract and price checks.

The provider of a qualified capability default is not extracted when a step overrides that
default with an unqualified model: provider preferences are a separate explicit setting.
Existing qualified configurations require no preference and retain their behavior.

The core owns this resolution. Planning and `doctor` use the same selection policy; doctor
reports adapter, key and local-program readiness afterward. It does not claim remote model
availability or validate settings that require actual workflow inputs.

## 2. Canonical model metadata and concrete routes

An `fx: routes/v1` entry may declare `model_id`, an unqualified canonical model identifier.
Its `route` remains qualified and names the provider's native model. An unqualified selector
matches `model_id`, or the native model when `model_id` is absent, exactly, for the selected
capability and provider. No prefix removal, fuzzy matching or model-family guessing occurs.
Several matching entries are ambiguous, even if their prices happen to agree.

`model_id` is selection metadata, not a route contract. It MUST NOT enter the route
fingerprint, step identity or call key. The existing concrete native model, provider and
completed contract remain authoritative ([identity.md](identity.md) §7–§10). Adding metadata
does not change existing qualified route fingerprints. A selection resolving to the same
concrete route shares its cache; another provider does not acquire its call cache.

When an entry extends its own qualified route to restate it, omitted `model_id` is inherited.
An entry extending a different qualified route does not inherit that model's identity: it
declares its own `model_id` or matches its own native model. Other `extends` and catalog
overlay rules, including `--routes` exclusion of built-ins, are unchanged.

Resolution provenance may be shown with the chosen model and provider: `step`,
`planning_project` or `workflow_home`, and the relevant configuration key. It carries no
absolute paths and is not identity. Existing model-independence checks are unchanged.

Runtime calls consume the already bound concrete route. Resume continues to replan current
configuration and compare plan digests; changing the resolved model or provider refuses an
existing incompatible run rather than silently switching or restoring previous preferences.
