# Issues

Known problems that are not fixed yet. Each says what was seen, what is known, and what would settle it.

## 1. FX keeps no record of what a provider says it billed

**Status:** open. A design gap, not a bug.

An answer holds only its files and its `data`: "no request id, no usage, no provider URL" ([providers.md](spec/providers.md) §4.4, and the §11 change notes). That rule is right for the answer, since a call record is replayed to node bodies and must not depend on which provider made it. But nothing else keeps the provider's usage either: not the call record, and not the run's events. FX reads the cost it needs and drops the rest.

So a bill cannot be explained after the fact:

- OpenRouter's `usage` splits a bill into image-output tokens, reasoning tokens, prompt tokens, and the upstream provider's own charge. FX keeps only the total.
- OpenAI reports token usage and no cost, so FX books an OpenAI image at its whole hold and keeps none of the tokens. Nothing then shows whether the hold covered the call.
- No request or generation id is kept, so a call cannot be looked up at the provider afterwards.

The per-size image prices of [providers.md](spec/providers.md) §10 could only be measured from an earlier engine's records, which kept the usage block. Issue 2 is open because FX's own run kept nothing.

**Proposal.** Keep answers as they are. Record what the provider reported on the run's `call` event: the token counts and the cost breakdown, as allowlisted numbers, and the provider's request id when it is a safe id ([providers.md](spec/providers.md) §8). None of it is a secret, none of it enters a call key, and a replay would carry no usage, since nothing was billed. Spec: [protocol.md](spec/protocol.md) (the `call` event), [providers.md](spec/providers.md) §4.4 and each provider's section of §9, and the events schema.

## 2. OpenRouter billed twice the image output for one call

**Status:** open, cause not identified.

**Seen.** FX's first live smoke run (2026-10-07) made one `image.generate` call on `openai/gpt-image-2.5-sunburst@openrouter`: 1024x1024, quality `max`, background `opaque`. OpenRouter reported a cost of $0.42164. That is twice the 7,024 image-output tokens the same size and quality have always cost (2 × 7,024 × $30 per million = $0.42144), plus the prompt.

**Every earlier record has the single count.**

- OpenRouter, 2026-09-09: a 1024x1024 `max` call at 7,024 image-output tokens ($0.210835), and a 1024x1536 `max` call at 5,488 tokens.
- OpenAI directly, up to 2026-09-29: 34 calls at 1024x1024 `max`, each 7,024 tokens, and 426 at 1536x1024 `max`, each 5,488.

**Ruled out:**

- **Another upstream.** OpenRouter routes this model only to OpenAI, at OpenAI's rates: $30 per million image-output tokens, $8 image input, $5 text input ([endpoint list](https://openrouter.ai/api/v1/images/models/openai/gpt-image-2.5-sunburst/endpoints)).
- **A retry by FX.** The run holds one reservation and one send.
- **A second image.** The adapter refuses an answer without exactly one image, and this one passed.
- **The prompt.** It was billed normally.
- **A published change.** OpenAI's changelog lists none since the model's release on 2026-09-08.

**Open candidates:**

1. **`moderation: low`.** FX always passes it to OpenAI through `provider.options` ([providers.md](spec/providers.md) §9.2). The earlier calls that billed the single count did not send it. It is the only request difference found, and no mechanism is known for it to double the output.
2. **An unannounced change since late September**, at OpenRouter (counting the output twice) or at OpenAI (the model drawing twice as much at `max`).

**Settles it:**

- **OpenRouter's activity page** for that generation shows its token breakdown: 14,048 image tokens points at the generation side (candidate 1 or OpenAI); 7,024 at a $0.42 cost points at OpenRouter's billing.
- **Two 1024x1024 `max` calls on OpenRouter**, one with `moderation: low` and one without (about $0.85), separate candidate 1 from candidate 2.
- **Once issue 1 is fixed**, any new call shows its breakdown in the run.

**What depends on it.** FX's OpenRouter image highs cover twice the output as a precaution. They are only holds, since OpenRouter reports its cost. If OpenAI itself now bills twice, the OpenAI and fal highs under-count by up to one output: a 1024x1024 generate would cost about $0.42 and be booked at $0.29 ([providers.md](spec/providers.md) §10, "An open risk").

## 3. Unversioned node exports in one module can share a cache identity

**Status:** open. Cause identified; no identity change implemented.

**Seen.** An offline image workflow defined `read_image` and `recolor` in the same Python
module, with no declared node versions or resources. Both took one `image` input. The
first node normalized a synthesized PNG without changing its bytes; the second should
have rotated its color channels. In a fresh cache, the first step recorded a cache miss
and the second a cache hit. The recolor body never executed, and both outputs were identical.

**Cause.** [`registry.rs`](crates/grida-fx-core/src/registry.rs) constructs an unversioned
type identity as `source:<digest(source)>`, where `source` contains the module's file
closure and resources. It does not include the selected export attribute. This follows
[identity.md](spec/identity.md) §6, which currently specifies that formulation. Two exports
from the same module with the same resources therefore have the same type identity.
With equal parameters and inputs, they also have the same node identity and cache key.
This is a contract gap rather than a viewer projection error.

**Workaround.** Put distinct unversioned node bodies in separate source files, as the
[image-recolor example](examples/image-recolor/) does. Declared versions also retain
the export path in the type identity. Separate step names or run folders do not fix
the unversioned collision.

**Required follow-up.** Ratify an identity that distinguishes the selected unversioned
export, update the specification and source/schema evidence together, and add a fresh-cache
regression where two exports in one module take identical inputs and produce different
outputs. Preserve cache/lock compatibility decisions explicitly; changing a public
identity is outside the viewer bootstrap.
