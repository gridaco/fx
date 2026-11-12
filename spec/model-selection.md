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

## 3. Explicit model import

`grida-fx models import <model> --provider gg --cap-usd <amount>` adds an image model from
Grida Models to the planning project without an engine release or handwritten route table.
The initial import profile supports GG image generation and editing only when the published
model and operation schemas demonstrate compatibility with FX's existing GG image adapter.
Unsupported interfaces, required inputs or answer shapes are refused before project writes.
Listing a model or belonging to the same family alone does not establish compatibility.

Import is an explicit network operation, separate from offline planning and running. Without
`--release <tag>` it resolves the latest published Models release and records the actual tag.
`--from <directory>` instead reads `SHA256SUMS`, `manifest.json`, `models.v1.json`,
`models.v1.schema.json` and `gg.operations.json` from that directory. It makes no network
requests. An explicit release must match the supplied manifest.

The Models manifest, release asset digests and schema are validated before use. Public
operation schemas are pinned by their digest in the imported table. Import does not retrieve
arbitrary schema references. Downloads are credential-free, bounded to 32 MiB per asset and
use HTTPS and the known public release/schema origins; bounded release-asset redirects are
allowed. The network switch remains authoritative.

Every imported image route requires a positive per-call `--cap-usd` with the existing USD
precision. This is a planning allowance, not a provider quote or spending approval. No price
or feature is copied merely because an older model supported it. Features come from the
intersection of published operation facts and guarantees the admitted FX adapter supports.
Unknown facts do not enable features. Import never generates media or writes credentials.

The command writes ordinary `fx: routes/v1` data under
`routes/models/<table-content-sha256>.yaml` and registers the file under the planning
project's `route_tables`. The table declares canonical `model_id`, native qualified routes,
explicit caps, features and the serving adapter's contract. Optional top-level `model_source`
records the public repository, actual release and the model-asset, schema and operation
SHA-256 digests. It is provenance, not identity.

All validation and configuration editing are prepared before writes. Files are published
atomically, route data before its configuration reference. Existing route files are not
overwritten. If configuration publication fails, an unregistered immutable table may remain;
it must not be removed when another import could already reference it. Reimporting a table
moves its registration to the end when necessary so the requested facts and cap take effect;
repeating an already effective import is idempotent. Imports use the existing overlay rules
and do not modify workflows,
selected defaults, provider preferences or existing run records. Configuration comments are
preserved; a configuration layout that cannot be edited safely is refused with correction
instructions before changes are published. Imported files are portable project source.

The setup command is CLI-only. Both SDKs consume the resulting project configuration and
route tables through their existing plan/run APIs; they do not implement resolution or a
separate configuration writer.
