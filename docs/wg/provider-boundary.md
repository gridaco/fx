# Provider semantics and managed execution

Status: boundary adopted under the owner's delegated design authority, 2026-11-11.
Implemented in source; the acceptance evidence below covers both SDKs. This clarifies decision 15 in the
[overview](overview.md) and the [AI provider contract](provider-contract.md).
The [adapter contract](../../spec/adapters.md) and node protocol are normative.
No new invocation command or generic node was needed; failure outcomes gained
optional diagnostic metadata.

## Problem

[Single-image mesh generation](https://github.com/gridaco/fx/issues/123) exposed
an abstraction error. FX implemented Tripo's multiview operation and required
named views in the provider-neutral `mesh.generate` capability. A restriction of
the selected endpoint became a restriction on the user's job. Tightening the
multiview adapter's validation was correct for that endpoint, but could not make
the single-image operation available.

The existing tests establish agreement between FX's capability, built-in node,
adapter and advertised features. That agreement alone cannot establish that a
useful provider operation is expressible. Nor should FX have to implement every
provider's schema before a user can use an operation.

Error fidelity is part of the same boundary. Before this change the Tripo adapter retained an
HTTP status and an integer provider code, but deliberately discarded provider
error text. A user therefore could not always learn the rejected field or the
provider's explanation from FX. Dynamic input support cannot depend on an error
experience that had not yet been implemented.

## Decision

FX owns managed execution. A provider owns the meaning and acceptance of its
native requests. Standard capabilities offer additional shared semantics;
their schemas must not become mandatory schemas for every provider operation.

| Boundary | Owner and obligation |
|---|---|
| Request identity, content-addressed files, cache and recorded history | The engine; identical inputs and adapter contracts have the identity the spec defines. |
| Admission, holds, settlement and retry decisions | The engine; the adapter supplies evidence about receipt and cost, and uncertainty remains explicit. A cap is an FX allowance, not a provider-enforced spending limit. |
| Submission, uploads, polling, result delivery and provider outcome interpretation | The adapter; it translates the provider protocol into the common request or long-job contract. |
| Accepted native fields, combinations, ranges and model-specific behavior | The provider; FX does not maintain an exhaustive copy of its input schema. |
| Standard capability semantics and answer checks | FX; callers opting into a standard capability receive its documented guarantees. |
| Useful diagnostics | The adapter extracts provider evidence; the engine preserves its origin, phase and safe detail across CLI, SDK and recorded inspection. |

### Standard and native operations

The standard path remains typed. For example, a standard mesh operation can
accept an image or named views and promise a model answer. Its adapter maps the
selected source to the appropriate provider operation. An adapter that supports
only one source form says so. Planning checks known incompatibilities; success
at planning is not a claim that the provider will accept the request.

The final extension path is a provider-native operation with arbitrary canonical
JSON and content-addressed file inputs. The engine must not enumerate that
operation's provider fields or require it to fit `mesh.generate`,
`image.generate` or another standard schema. Provider additions can pass through
without an engine release when the adapter's wire mapping already supports them.
An optional description or schema may help authoring; it is not a mandatory
engine-maintained provider schema on this path.

This path uses adapter-declared capabilities outside the standard vocabulary,
already permitted by the [project adapter contract](../../spec/adapters.md#9-project-adapters).
A project node can call one through the existing managed-call interface. Do not
add a second scheduler, billing system or raw-HTTP execution mechanism. Do not
add a generic invocation node or CLI flag until an executable example shows
that the existing node and adapter composition is insufficient.

Choosing the native path is explicit. A failed standard request does not
silently fall back to a native operation or another endpoint. A standard call's
validation cannot be disabled while still claiming that capability's guarantees.

"Arbitrary input" means freedom from FX's provider schema. The execution
envelope still requires canonical values, resolvable file references, a bound
route, an adapter identity, a price or cap, credential separation and the common
outcome contract. Credentials and signed URLs do not become ordinary recorded
parameters. The adapter may reject a request it cannot serialize or transport,
and identifies that as an adapter limitation rather than a provider rejection.

### Dynamic data does not supply a protocol

Accepting a field does not teach an adapter which endpoint to submit to, how to
upload a file, what constitutes acceptance, where to find a task ID, when a job
has ended or which output to download. A body-only passthrough would leave those
responsibilities unresolved.

A native operation therefore still has an adapter binding that understands its
wire and lifecycle. Providers with a uniform invocation protocol can support
many model IDs or operation IDs with one binding. A genuinely different
protocol needs adapter code, which may be supplied by a project without changing
the engine. The goal is independence from FX releases, not the claim that an
arbitrary URL automatically has safe long-job semantics.

For Tripo, the image and multiview operations can share the existing upload,
submit-outcome, task collection and model-delivery machinery while differing in
the submission path and input mapping. Native and standard bindings should
reuse that machinery. This does not require generating adapters from provider
documentation or introducing a general-purpose mapping language.

### Provider errors remain provider errors

Preserve FX's stable execution outcome and carry provider diagnostics alongside
it. A machine consumer must be able to distinguish an engine validation error,
an adapter limitation, a transport failure, a provider rejection and a provider
task failure without parsing an English sentence.

The diagnostic design must retain, when supplied and safe:

- the provider and operation, the phase such as upload, submit, poll or download,
  and the source of the error;
- the HTTP status, provider error code, field name and provider explanation;
- a provider request or task ID that can be used for reconciliation;
- whether detail was redacted, truncated, unavailable or unreadable.

The schema is `adapter_diagnostic` in the node protocol. Python authoring uses
`AdapterDiagnostic`; TypeScript authoring uses an object with idiomatic camelCase
HTTP/ID fields. Recorded diagnostics use the schema's lower_snake_case names.
The same diagnostic must reach terminal text, SDK errors and recorded inspection.
Older records remain readable with their existing reasons; they do not acquire
provider detail that was never recorded.

Extract known error fields rather than dumping response bodies. Bound and
redact the selected explanation, including known credentials, transient upload
tokens, signed URLs and echoed sensitive inputs. Provider-specific extraction
must be reviewed and tested; a regex redactor alone is not proof that arbitrary
response text is safe. Where a safe explanation cannot be extracted, explicitly
report that omission and keep the available status, code and reconciliation ID.
Input-echo protection applies to identifier fields and operation names too, even when
they contain only identifier characters. Unsafe optional metadata is withheld.
If the provider supplied no useful explanation, FX must say so rather than
inventing one. Forwarded text is untrusted diagnostic data, never instructions.

A provider rejection does not imply that nothing was billed. HTTP status,
diagnostic text, retry eligibility and settlement are separate facts. An
ambiguous submit remains uncertain even if an error body looks informative.
Collection never resubmits. These rules hold equally on the native path.

### Validation serves a declared guarantee

Keep checks needed for canonical identity, readable files, serialization,
transport and credential boundaries, budgeting and lifecycle integrity. Keep
the semantic checks callers opt into through a standard capability.

On the native path, do not reject an unknown provider field, newly accepted
combination or expanded range solely because FX's snapshot predates it. Known
provider restrictions may produce useful authoring advice, but must not become
an exhaustive engine schema that a caller cannot get past. Restrictions needed
by the adapter's actual encoding or outcome interpretation remain explicit
adapter limits. A native result gets the declared file and serialization checks;
it does not implicitly get a standard capability's semantic guarantees.

## Delivery and evidence

1. The [native harness](../../fixtures/native/README.md) and the Python and
   TypeScript native-engine integration tests exercise custom fields and a file
   through the real engine and both SDK hosts. They cancel after submission,
   resume by collection, distinguish both caches and change fields, file bytes
   and operation names to check identity.
2. The adapter and node protocols define optional structured diagnostics. Both
   SDKs serialize them, capability errors retain them, and node-failure events,
   CLI inspection/projection, observation and browser projections preserve them.
   Default exports withhold them. An uncertain job keeps its diagnostic alongside
   its note for later reconciliation. Legacy reason-only hosts remain readable.
3. Standard mesh generation accepts exactly one image or named views. Tripo
   shares upload, task submission and collection across both operations, with
   unchanged multiview wire bodies and identities. The built-in GG route remains
   multiview-only, and image input is refused in its plan. Provider replay tests
   cover image submission, rejection, malformed/ambiguous responses and task
   failure. Runtime call tests cover the real adapter with the store, ledger,
   retry owner and offline call replay.
4. Tripo forwards integer face counts without a stale provider range; local mesh
   limitations identify the adapter and validation phase. The pinned model checks
   cover both Tripo operations. Native operations remain
   available through project adapters rather than an exhaustive engine schema.
   Other built-in adapters can adopt structured diagnostics independently; until
   then they retain their legacy reasons. New wire protocols still need adapter
   code, not an engine capability-schema release.

Each implementation change starts in `spec/`, updates `llms.txt` and the affected
SDK and guide surfaces, and adds readiness evidence. A refreshed pinned provider
snapshot can expose coverage changes; this is not a live provider-monitoring
promise and must not enable new behavior automatically.

## Acceptance and compatibility

- A custom native field reaches the synthetic adapter unchanged, is recorded,
  affects identity, and does not require a standard capability schema change.
- A changed input file or operation affects the managed call's identity; an
  identical call replays offline. Adapter behavior and operation selection are
  identified by the existing request and route-contract rules. Do not normalize
  native and standard requests into one cache key as part of this work.
- A deterministic provider input rejection exposes its origin and safe detail
  without an automatic new paid attempt. A malformed error body remains
  attributable and cannot conceal whether submission is uncertain.
- A submitted job resumes by collection alone; an uncertain submit stops for
  reconciliation. Both paths preserve existing reservation and settlement rules.
- Diagnostics containing made-up keys, signed URLs, local paths, control
  characters, media payloads and echoed inputs cannot leak them into portable
  records, SDK errors or browser documents.
- Existing multiview requests, call-cache entries and outstanding jobs are
  covered by explicit compatibility tests. Any necessary node or adapter version
  change and its effect on replay and resume are documented before shipping.
- SDK, CLI and read-only inspection report the same source and outcome. Planning
  reports only what it can know; provider acceptance is established by execution.

The implementation uses the existing adapter protocol as its extension foundation.
The native harness, both SDK integration suites, Tripo replay tests and runtime
managed-call tests are the executable evidence. No provider is contacted by them.

Review regressions also inject file contents into each metadata field in both SDKs,
including uncertain jobs and exports with diagnostic disclosure but values withheld.
Provider replay tests cover face counts above the former cap for both topologies;
the real-adapter managed-call test uses 40,000 faces. A local mesh refusal reaches
Python failure, observation and inspection with adapter/validate provenance and $0 cost.
