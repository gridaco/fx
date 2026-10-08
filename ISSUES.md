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
