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
- **Route contract policy.** A contract takes only `adapter`, `adapter_behavior` and `provider_options`; another member is refused as `<route> is not a <capability> route this adapter serves: the contract takes no member <m>`.
- **Structured output and agent turns** are [OpenRouter's](../openrouter/README.md) in every message, schema, tool, picture, refusal and answer, with these differences:
  - The body is `{model, messages, response_format, max_tokens}` for structured output, `max_tokens` being the request's, else 16 000; and `{model, messages, tools, tool_choice, max_tokens?}` for a turn, `max_tokens` only when the request gives one. The provider options follow, and are refused before the other values.
  - Structured pictures are always fitted into 1600 × 1600, flattened on `matte` and sent as PNG; a contract has no `pictures` member to send them as they are. No picture carries a `detail`.
  - The order of refusals before sending is: the contract; the request against its capability; the key; the provider options; the values and files of the request; the body's size.
- **Answers.** Structured output answers `data: {"json": <value>}`, and a turn `data: {"text", "tool_calls": [{"id", "name", "arguments": <object>}]}`, from the first choice's message. A 2xx whose body is not a JSON object, has no message or holds no answer is a structural failure: `Failed { cost: None, retryable: true }` ([adapters.md](../../../../spec/adapters.md) §3.4), `<label> returned invalid JSON`, `<label> returned no message`, and OpenRouter's sentences for the rest.
- **Check:** structured output's value against the request's original schema (draft 2020-12, `format` not asserted), refused as `<path>: <message>`; a turn has none of its own, and the engine checks its shape. This is the every-route check of each capability ([capabilities.md](../../../../spec/capabilities.md) §4, §5), so a stand-in's answer on a declared provider's route, which a stand-in run holds to that check ([adapters.md](../../../../spec/adapters.md) §1), is judged the same.
- **Cost:** none is read ([adapters.md](../../../../spec/adapters.md) §5): every answer and every failure costs `None`, so each attempt is booked at its whole hold, or its cap on a capped route. The answer's usage is the completion's `usage`: `prompt_tokens`, `prompt_tokens_details.cached_tokens` and `completion_tokens`, each when given.
- **Billing:** dollars, to the account of the declared key ([adapters.md](../../../../spec/adapters.md) §5). No declaration claims a plan.
- **Statuses:** [adapters.md](../../../../spec/adapters.md) §3.2 and §3.3 as written, with no override:
  - a 3xx is `Failed { cost: None, retryable: false }`: `<label> was redirected (HTTP <n>); check <VARIABLE>`, the declaration's base-URL variable, or `check providers.<name>.base_url` for a declaration without one;
  - 408 and 429 are `NotReceived`, the provider's `retry-after-ms` or `retry-after` setting the wait;
  - any other 4xx is `Failed { cost: None, retryable: false }`: no declared provider is known to document its refusals as unbilled;
  - a 5xx, and any other status, is `Failed { cost: None, retryable: true }`.
- **Reasons:** the labels are `<name> structured generation` and `<name> tool loop`, `<name>` the declared provider's. A status's reason is `<label> returned HTTP <n>`, with the safe detail of an OpenAI-style error envelope ([adapters.md](../../../../spec/adapters.md) §7), ` (request <id>)` when the response carries a safe request id, and for a 408 or a 429 `; retry-after <seconds>` when the header gives a short number. The declared key is redacted from every reason.
