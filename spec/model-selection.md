# Model selection and provider preferences

Model selection is authoring configuration. The engine resolves it to an ordinary concrete
route before pricing, approval, identity and execution. For supported providers, preparing a
missing image-model binding may read provider metadata over the network (§3). The planning
core itself remains offline. Discovery never generates an asset or authorizes spending.
The words MUST and MUST NOT are used as in RFC 2119.

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
   When no binding exists, the engine may prepare one through the selected provider's
   supported metadata lookup (§3). Ambiguous bindings are refused without discovery.
   Credentials do not choose a provider, and a missing key does not cause another provider
   to be selected.
5. Apply the existing route's feature, input-count, contract and price checks.

The provider of a qualified capability default is not extracted when a step overrides that
default with an unqualified model: provider preferences are a separate explicit setting.
Existing qualified configurations require no preference and retain their behavior.

The core owns selection and concrete-route resolution; the engine prepares provider metadata
before invoking it. Planning and `doctor` use the same selection policy; doctor reports
adapter, key and local-program readiness afterward. Finding model metadata is not a provider
health check and does not prove that a generation will succeed. `doctor <target>` prepares
missing used bindings through the same discovery path; `doctor` without a target stays local.
Doctor does not validate settings that require actual workflow inputs.

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

## 3. Selected-provider image discovery

The engine prepares a missing `image.generate` or `image.edit` binding on `gg` or
`openrouter` by reading that provider's model metadata. This happens automatically during
normal planning and run preparation: no model import or manual synchronization is required.
It asks only the provider already selected by the qualified selector or provider preference.
It does not search for another provider, change the authored model ID, upgrade to a newer
model or make a paid fallback call.

Authored route tables remain authoritative. Discovery MUST NOT replace an admitted binding,
repair an ambiguous binding, or weaken a route's authored feature, input, contract or price
checks. An explicit `--routes` invocation disables automatic discovery, retaining that
invocation's isolated catalog. Other providers and capabilities continue to use admitted
route tables, including project tables for models FX does not ship. A project provider or
adapter does not need a model-discovery service or a Grida Models catalog.

Model discovery and pricing discovery are optional provider-integration capabilities, not
requirements of a provider declaration. FX's route-price contract is the common planning
boundary regardless of whether its facts come from a shipped table, a project table or a
provider lookup. Prices may remain FX-maintained estimates or user-authored allowances;
neither discovery nor execution requires a provider's request-estimation API. A provider's
illustrative scenario is not a general rate formula; using an estimate requires an
interpretation that applies to the route's supported requests.

Provider metadata is normalized into ordinary concrete routes before the offline planner
runs. A provider's published model facts are admitted only for the request and response
wire the installed adapter implements. An unsupported wire, missing image capability or
unknown required feature is a refusal; metadata cannot install an adapter or execute code.
Discovery also refuses published requirements the installed wire cannot honor: generation
endpoints requiring reference inputs, and GG output-format restrictions excluding PNG.
Model matching remains exact under §2. The serving adapter's identity completes the route
contract under [identity.md](identity.md) §7.

Lookups are read-only provider GET requests over the engine's transport, each limited to
4 MiB and a 15-second deadline. They generate
no assets, create no provider jobs and incur no generation charge. They may use only the
selected provider's declared credential, with the same base-URL origin confinement and
redaction as other provider requests ([providers.md](providers.md) §1,
[adapters.md](adapters.md) §2). They do not follow redirects or retry. With
`GRIDA_FX_NETWORK=off`, no lookup request is sent. A lookup failure does not change the
provider or silently admit unsupported facts; without sufficient locally available metadata
or an authored route, resolution is refused with the missing requirement.

This path reads the provider APIs. It does not download Grida Models releases at runtime.
The existing build-time Models projection continues to check shipped routes. A public
custom-resolver API is deferred; existing project route tables and adapters remain the
extension paths.

Execution compatibility and cost forecasting are independent. A discovered binding MUST identify
a supported adapter contract and satisfy required features, but MAY have no price. Authored
routes retain deferred adapter checks: they can plan without an installed adapter, and execution
refuses when none serves them. Shipped, authored and discovered routes share the optional
pricing contract. A missing price is an unknown forecast,
never zero. An explicit `price: null` clears an inherited price; omission beside `extends`
continues to inherit it. A supplied price or range is an estimate, not a guaranteed invoice.
An existing `price.cap_usd` is an authorization allowance, not a forecast.

Compatible facts with no applicable price are cached and retained in prepared plans normally.
A provider's illustrated scenario is usable as a forecast only when its assumptions apply.
A known tariff without the request quantities needed to use it does not establish a total.
No provider estimation endpoint, manual synchronization or per-model allowance is required.
Authored entries still replace whole entries; discovery does not merge into them implicitly.

`--require-estimates` on planning and execution refuses missing forecasts for uncached paid
work, including capped-only routes. The equivalent SDK option has the same meaning. It
requires forecast coverage, not exact future billing. Free stand-in execution is exempt.
Coverage conservatively checks potentially paid calls of uncached steps; planning cannot prove
that arbitrary node bodies will replay every request from the managed-call cache.
Ordinary planning succeeds with unknown forecasts and presents cost incompleteness as information.

Unknown forecasts propagate through instances, pending repeats, phases and totals. A partial
subtotal MUST NOT be labeled a complete total. Graphs containing unknown forecast components
use `fx-graph-v2`; their affected money objects have null `low_usd` and `high_usd`, numeric
`known_low_usd` and `known_high_usd`, and positive integer `unpriced_calls`. Price documents with an unknown aggregate add `kind: fx-price-v2` and use the same amount
objects. Fully forecastable graphs retain `fx-graph-v1`. Readers support existing records; pricing changes alone do not
change route fingerprints, call keys or generated-result cache identity.

One prepared plan admits at most 128 discovered image bindings, matching its snapshot
reader's limit. A larger selection is refused before an unreadable snapshot can be produced.

## 4. Metadata cache and prepared plans

The planning project's `.fx/models/` is an optional metadata cache, separate from the paid
call and result store. It holds sanitized normalized routes, a connection-scope hash and a
fetch timestamp, never raw responses, credentials, credential-bearing URLs or headers.
Different provider connections do not silently share an entry. Cache failures do not prevent
an otherwise valid online lookup.

An entry is fresh for one hour. With the network on, an expired entry is refreshed before
use; it may be used on a transient lookup failure. With `GRIDA_FX_NETWORK=off`, a usable
cached entry may be used regardless of age, without sending anything. An explicit provider
response that the selected model does not exist MUST NOT silently fall back to stale data.
Without usable metadata or an authored route, offline resolution is refused. The cache is
an optimization, not an offline-availability guarantee or a source of paid answers.

A prepared SDK plan carries its engine-resolved model routes independently of this cache.
Running that plan reuses its bound discovered routes, not a fresh model lookup; deleting or
expiring the metadata cache does not substitute a different model or contract. The engine
still checks current authored configuration and the usual run admission rules. SDKs transport
the normalized route data; only the engine validates and resolves it.

A CLI resume may reuse the run's recorded discovered model facts without consulting the
metadata cache or provider. Fresh plans obtain current metadata under the policy above;
all resumes retain the current-configuration and
plan-digest checks in §2.
