# OpenAI-compatible providers

The reference for the adapters of the `openai-compatible` family, under the adapter contract, [spec/adapters.md](../../../../spec/adapters.md), whose sections it cites; it is normative for these adapters. [spec/providers.md](../../../../spec/providers.md) §1 holds a declaration, and §2 the family's adapters.

| Capability | Contract `adapter` | `adapter_behavior` |
|---|---|---|
| `structured.generate` | `openai-compatible-structured` | `1` |
| `agent.turn` | `openai-compatible-tool-loop` | `1` |

- **Providers:** every provider the planning project declares with `family: openai-compatible` ([providers.md](../../../../spec/providers.md) §1), by its declared name: `<model>@acme` for a provider declared as `acme`. Each such provider gets both adapters, each the default of its capability there, so a route of its own table that names no `adapter` is filled in with the adapter's name and behaviour, and one that names the adapter without `adapter_behavior` with its behaviour ([identity.md](../../../../spec/identity.md) §7). The model is the route's.
- **Capabilities:**
  - `structured.generate` (`POST {base}/chat/completions`, a strict `json_schema` response format). Contract `adapter`: `openai-compatible-structured`, behaviour `1`, the number.
  - `agent.turn` (`POST {base}/chat/completions`, strict function tools). Contract `adapter`: `openai-compatible-tool-loop`, behaviour `1`, the number.
- **The wire** is the [OpenAI Chat Completions API](https://platform.openai.com/docs/api-reference/chat/create), and only the members of it that FX's structured output and agent turns need: `model`, `messages` (text, and pictures as `image_url` parts holding data URLs), `max_tokens`, `response_format` of type `json_schema` with `strict: true`, `tools` of type `function` with `strict: true`, and `tool_choice` (`required` or `auto`). Nothing chosen for one provider or model is sent: no routing, no reasoning effort, no picture `detail`, no sampling. A provider's or a model's own parameters go in `provider_options.<name>`.
- **Base URL:** the declaration's: its variable's value when it is set, else its default ([providers.md](../../../../spec/providers.md) §1).
- **Credential:** as the declaration says, `<header>: <scheme> <key>`, or `<header>: <key>` with no scheme. A provider declared without a credential is sent none, and needs no key. A provider declared with one whose key is not set is refused before sending: `<VARIABLE> is not set`.
- **Request bodies:** pictures go inline as data URLs. A request body over 200 MiB is refused before sending: `request body exceeds 200 MiB`.
- **Limits:**

  | Capability | Deadline | Response cap |
  |---|---|---|
  | Structured | 1800 s | 16 MiB |
  | Agent turns | 600 s | 16 MiB |

- **Provider options** ([adapters.md](../../../../spec/adapters.md) §8): the parameters of `provider_options.<name>`, for the route's provider, are added at the end of each body, after FX's members, and are refused, as the first of the values below, when they set a member FX owns:
  - structured output: `model`, `messages`, `response_format`, `max_tokens`, `max_completion_tokens`, `stream`, `n`, `tools` and `tool_choice`;
  - agent turns: `model`, `messages`, `tools`, `tool_choice`, `max_tokens`, `max_completion_tokens`, `stream`, `n` and `response_format`; the engine's agent loop sends a turn none, so an agent run's turns take only the route's.

  `max_completion_tokens` would set the token limit `max_tokens` sets, `stream` would answer in events the adapter does not read, `n` would answer more than one choice, and `tools`, `tool_choice` or `response_format` would change what the answer holds.
- **Route contract policy.** Both capabilities take `adapter`, `adapter_behavior`, `provider_options`, `image_detail`, `token_limit_field` and `billing`. Structured output also takes `pictures`. Another member is refused as `<route> is not a <capability> route this adapter serves: the contract takes no member <m>`. Invalid settings are refused before sending, with the same prefix followed by their reason.
  - `image_detail` is `auto`, `low`, `high` or `original`, sent as each picture's `detail`. Absent or `null` sends no detail; the provider decides whether it supports an explicitly chosen value.
  - `pictures: unchanged`, on structured output only, sends each context picture's original bytes and media type. Absent or `null` keeps the fitting and flattening below. Agent turns already send their original pictures.
  - `token_limit_field` is `max_tokens` (the default) or `max_completion_tokens`. FX sends its token limit under exactly that name, and still owns both names in provider options. It never retries a refused request using another spelling.
  - Reasoning settings use the provider's own parameter names in `provider_options.<name>`; the family adds no reasoning parameter of its own.
- **Structured output and agent turns** are [OpenRouter's](../openrouter/README.md) in every message, schema, tool, picture, refusal and answer, with these differences:
  - The body is `{model, messages, response_format, max_tokens}` for structured output, the token limit being the request's, else 16 000; and `{model, messages, tools, tool_choice, max_tokens?}` for a turn, a token limit only when the request gives one. `token_limit_field` selects its name. The provider options follow, and are refused before the other values.
  - Structured pictures are fitted into 1600 × 1600, flattened on `matte` and sent as PNG unless `pictures: unchanged` is set. Pictures carry `detail` only when `image_detail` is set.
  - The order of refusals before sending is: the contract; the request against its capability; the key; the provider options; the values and files of the request; the body's size.
- **Answers.** Structured output answers `data: {"json": <value>}`, and a turn `data: {"text", "tool_calls": [{"id", "name", "arguments": <object>}]}`, from the first choice's message. A 2xx whose body is not a JSON object, has no message or holds no answer is a structural failure: `Failed { cost, retryable: true }` ([adapters.md](../../../../spec/adapters.md) §3.4), preserving a valid opted-in reported cost, else unknown. Its reasons are `<label> returned invalid JSON`, `<label> returned no message`, and OpenRouter's sentences for the rest.
- **Check:** structured output's value against the request's original schema (draft 2020-12, `format` not asserted), refused as `<path>: <message>`; a turn has none of its own, and the engine checks its shape. This is the every-route check of each capability ([capabilities.md](../../../../spec/capabilities.md) §4, §5), so a stand-in's answer on a declared provider's route, which a stand-in run holds to that check ([adapters.md](../../../../spec/adapters.md) §1), is judged the same.
- **Cost:** by default none is read ([adapters.md](../../../../spec/adapters.md) §5), so each attempt is booked at its whole hold, or its cap on a capped route. An optional `billing` object takes only these members; absent or `null` members keep the defaults:
  - `cost: usage.cost` opts into the response body's numeric `usage.cost` as the full final charge in US dollars, on answers, structural failures and non-2xx responses alike. It must be the provider's documented total, not a token estimate or one part of a charge. A valid non-negative number is rounded up to micro-dollars and wins over an unbilled-status rule, even when it is zero. A missing or `null` value is unknown; any other invalid value (including a string, boolean, negative number or an amount too large to represent) remains unknown and prevents an unbilled-status inference. Billing reads strict I-JSON with no duplicate keys; an ambiguous or unreadable body never proves a missing charge.
  - `unbilled_statuses: [400, ...]` declares the provider's documented unbilled refusal statuses. Values are distinct integers from 400 through 499, excluding 408 and 429. On one of these statuses, no reported cost means `Some(0)`; an invalid opted-in reported cost or an unreadable body remains unknown. An empty body carries no reported cost. These statuses still fail without retrying. The rules consult `usage.cost` only when `cost: usage.cost` is set; without it, cost-looking fields are ignored.
  - No other cost path, currency conversion, error-text heuristic or retry policy is supported. A provider needing those uses a project adapter. Unknown costs are booked at the whole hold or cap.

  The answer's token usage is the completion's `usage`: `prompt_tokens`, `prompt_tokens_details.cached_tokens` and `completion_tokens`, each when given.
- **Billing:** dollars, to the account of the declared key ([adapters.md](../../../../spec/adapters.md) §5). No declaration claims a plan.
- **Statuses:** [adapters.md](../../../../spec/adapters.md) §3.2 and §3.3, with a conservative override for 408: wire compatibility does not prove that a timed-out request did no work.
  - a 3xx is `Failed { cost, retryable: false }`: `<label> was redirected (HTTP <n>); check <VARIABLE>`, the declaration's base-URL variable, or `check providers.<name>.base_url` for a declaration without one;
  - a 408 is `Failed { cost, retryable: true }`: the request may have arrived and been billed, so the engine settles the reported cost, else the whole hold or cap, and reserves a new attempt before sending again;
  - a 429 is `NotReceived`, the provider's `retry-after-ms` or `retry-after` setting the wait. With `billing.cost` enabled, a reported cost (including zero), an invalid cost or an unreadable nonempty body instead makes it `Failed { cost, retryable: true }`, so no possible charge disappears into an unbilled resend;
  - any other 4xx is `Failed { cost, retryable: false }`;
  - a 5xx, and any other status, is `Failed { cost, retryable: true }`.
  - Every failure's `cost` follows the billing policy above, including a redirect's; it is `None` by default. A billed retry uses the engine's ordinary backoff, not a provider wait carried by `NotReceived`.
- **Reasons:** the labels are `<name> structured generation` and `<name> tool loop`, `<name>` the declared provider's. A status's reason is `<label> returned HTTP <n>`, with the safe detail of an OpenAI-style error envelope ([adapters.md](../../../../spec/adapters.md) §7), ` (request <id>)` when the response carries a safe request id, and for an unbilled 429 `; retry-after <seconds>` when the header gives a short number. The declared key is redacted from every reason.

The timeout accounting correction and optional settings keep behaviour `1`: absent settings change no request bytes or successful answer, so existing cached answers remain valid. Each explicit wire or billing setting is part of the route contract and therefore its fingerprint ([identity.md](../../../../spec/identity.md) §7). Existing settlements are never rewritten.
