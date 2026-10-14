# OpenRouter

The reference for FX's adapters on `openrouter`, under the adapter contract, [spec/adapters.md](../../../../spec/adapters.md), whose sections it cites; it is normative for these adapters. [spec/providers.md](../../../../spec/providers.md) §1, §2 and §3 hold their key and endpoint, the built-in adapters and the routes FX ships.

| Capability | Contract `adapter` | `adapter_behavior` |
|---|---|---|
| `image.generate`, `image.edit` | `openrouter-image` | `"1"` |
| `image.generate`, `image.edit` | `fx-openrouter-image-v1` | `"3"` |
| `structured.generate` | `openrouter-structured` | `1` |
| `agent.turn` | `openrouter-tool-loop` | `1` |
| `music.generate` | `openrouter-music` | `1` |

- **Capabilities:**
  - `image.generate` and `image.edit` on any model (`POST {base}/images`). Contract `adapter`: `openrouter-image`, behaviour `"1"`, the string: the default of both capabilities, which serves a route whose contract names no adapter ([providers.md](../../../../spec/providers.md) §2).
  - `image.generate` and `image.edit` on GPT Image (`POST {base}/images`). Contract `adapter`: `fx-openrouter-image-v1`, behaviour `"3"`: only a route that names it, as the built-in routes do.
  - `structured.generate` (`POST {base}/chat/completions`, a strict `json_schema` response format). Contract `adapter`: `openrouter-structured`, behaviour `1`.
  - `agent.turn` (`POST {base}/chat/completions`, strict function tools). Contract `adapter`: `openrouter-tool-loop`, behaviour `1`.
  - `music.generate` (`POST {base}/chat/completions`, `stream: true`, audio `mp3`). Contract `adapter`: `openrouter-music`, behaviour `1`.
- **Credential:** `authorization: Bearer <OPENROUTER_API_KEY>`.
- **Request bodies:** pictures go inline as data URLs. A request body over 200 MiB is refused before sending.
- **Limits:**

  | Capability | Deadline | Response cap |
  |---|---|---|
  | Images | 600 s | 64 MiB |
  | Structured | 900 s for `openai/gpt-6-astra`, else 1800 s | 16 MiB |
  | Agent turns | 600 s | 16 MiB |
  | Music | 900 s | 128 MiB |

- **Provider options** ([adapters.md](../../../../spec/adapters.md) §8): the parameters of `provider_options.openrouter` are added at the end of each body, after FX's members, and are refused, as the first of the values below, when they set a member FX owns:
  - images on any model: `model`, `prompt`, `input_references`, `n`, `response_format` and `stream`;
  - images on GPT Image: `model`, `prompt`, `n`, `provider`, `size`, `quality`, `background`, `input_references`, `response_format` and `stream`;
  - structured output: `model`, `messages`, `response_format`, `provider`, `reasoning`, `max_tokens`, `stream`, `n`, `tools` and `tool_choice`;
  - agent turns: `model`, `messages`, `tools`, `tool_choice`, `provider`, `reasoning`, `max_tokens`, `stream`, `n` and `response_format`; the engine's agent loop sends a turn none, so an agent run's turns take only the route's;
  - music: `model`, `messages`, `modalities`, `audio`, `stream` and `n`.

  OpenRouter's own routing is `provider`, which FX sets for structured output and agent turns from the contract's `request_policy` (below), and for GPT Image itself: provider options cannot add to it there. Images on any model and music send no `provider`, so their options may give one.
- **Route contract policy.** The structured and agent contracts take only `adapter`, `adapter_behavior`, `request_policy`, `provider_options` and (structured only) `pictures`; another member is refused as `<route> is not a <capability> route this adapter serves: the contract takes no member <m>`, and so is a bad value, with its sentence after the colon. `request_policy` (optional) gives:
  - `provider`: `require_parameters` must be true, and is the default (`request_policy.provider.require_parameters must be true`); `only` is a non-empty list of distinct provider slugs (`request_policy.provider.only is a non-empty list of distinct provider slugs`); `allow_fallbacks` is a boolean (`request_policy.provider.allow_fallbacks is true or false`);
  - `reasoning.effort`: `request_policy.reasoning.effort is one of none, minimal, low, medium, high, xhigh, max`;
  - `image_detail`, which applies to every picture: `request_policy.image_detail is one of auto, low, high, original`.

  Objects must be objects (`request_policy is an object`, `request_policy.provider is an object`, `request_policy.reasoning is an object`) and take no other member (`<where> takes no member <m>`). `pictures: unchanged` sends structured pictures as they are (`pictures is "unchanged" or absent`). Otherwise each picture is fitted into 1600 × 1600, flattened on `matte`, and sent as PNG.
- **Images on any model** (`openrouter-image`). The adapter sends what OpenRouter's image models share, and nothing FX chose for one model: no quality, no size, no moderation, no routing. A parameter of the model's own goes in `provider_options.openrouter`.
  - Refused before sending, in this order:
    - provider options (above);
    - a blank `prompt`: `an image call needs its prompt as text`;
    - a mask: `OpenRouter's generic image adapter sends no mask: give the model's own parameter in provider_options.openrouter`;
    - `background: transparent`: `OpenRouter's generic image adapter sends no transparent background: give the model's own parameter in provider_options.openrouter`; a `background` other than `auto`, `opaque` or `transparent`: `background must be auto, opaque or transparent`;
    - a `size` that is not `auto` or `<W>x<H>`: `size must be auto or WIDTHxHEIGHT`; an exact size: `OpenRouter's generic image adapter sends no exact size: give the model's own parameter in provider_options.openrouter`;
    - references on a generate, on a route that does not declare the feature `image_input`: `<route> does not support image_input`, the sentence a plan gives ([capabilities.md](../../../../spec/capabilities.md) §1), for a call the plan could not judge;
    - more input pictures than the route's `max_input_pictures`, when it declares one: `<route> takes at most <n> input pictures, not <count>`;
    - a picture that is not an `image/*` file: `<member> is <kind>, not a picture` (`image`, or `references[<i>]` counted from 0);
    - a body over 200 MiB: `request body exceeds 200 MiB`.
  - The body is `{model, prompt, input_references?}`, then the provider options. `input_references` is sent when the call has pictures: an edit's `image`, then `references`, or a generate's `references` on a route that declares `image_input`, each `{"type": "image_url", "image_url": {"url": <data URL>}}`. `size: auto`, `background: auto` and `background: opaque` are not sent: the model draws its own default, and the check holds an opaque request to an opaque picture.
  - The answer is `data[0].b64_json`, strict base64, never a URL. `data` holding anything but one object is `OpenRouter image generation returned no single image`, `Failed { cost, retryable: false }`: FX asked for no count, so another attempt would draw as many. The bytes decide the kind (`media_type` is not read): a PNG is kept as it is, and a JPEG, WebP or GIF (its first frame) is decoded and encoded as PNG, the same pixels. Bytes that do not decode are `OpenRouter's image is not decodable`, and a picture that cannot be encoded is `OpenRouter's image could not be encoded as PNG`, each `Failed { cost, retryable: true }`. The answer is file `image` (`image/png`), with `data: null`.
  - The check is every image route's ([capabilities.md](../../../../spec/capabilities.md) §2): a PNG that decodes, opaque when asked for.
  - Facts: [Grida Models](https://github.com/gridaco/models) `v1.20261013.6` records no request parameters on OpenRouter's image models but GPT Image's, and, on 13 of its 17 OpenRouter image bindings, the number of input pictures each takes (`references.max`, 1 to 16); the endpoint and its answer are the ones the GPT Image adapter reads.
- **Images on GPT Image** (`fx-openrouter-image-v1`). Refused before sending, in this order:
  - provider options (above);
  - a blank `prompt`: `an image call needs its prompt as text`;
  - a mask: `OpenRouter image generation has no masked-edit route`;
  - `background: transparent` (the route serves no alpha): `OpenRouter image generation does not support transparent backgrounds`; a `background` other than `auto`, `opaque` or `transparent`: `background must be auto, opaque or transparent`;
  - a `size` other than `auto` and `1024x1024`, `1152x2496`, `1712x2560`, `2064x1008`, `2496x1152`, `2560x1440`, `2560x1712`: `OpenRouter serves no exact size <size>`;
  - references on a generate: `OpenRouter image generation takes no references`;
  - more than 16 pictures: `OpenRouter image edits support at most 16 input images`;
  - a picture that is not an `image/*` file: `<member> is <kind>, not a picture` (`image`, or `references[<i>]` counted from 0);
  - a body over 200 MiB: `request body exceeds 200 MiB`.

  The body is `{model, prompt, n: 1, provider: {allow_fallbacks: false, options: {openai: {moderation: low}}}, size?, quality: max, background, input_references?}`, then the provider options. The answer is `data[0].b64_json`, never a URL. A declared `media_type` must be a non-blank, parameter-free `image/png`, `image/jpeg` or `image/webp` (ignoring case; `image/jpg` and `null` are refused as structural failures); without that member the kind is sniffed from the bytes. The kind is kept as declared or sniffed, and the check refuses anything but PNG.
- **Structured output.**
  - The schema is a `json` file or an inline object. It is sent after local `$ref` inlining and strict canonicalization: `default` and the assertions strict mode refuses are dropped at schema positions, and `required` lists every property and `additionalProperties` is false. Schema positions are the root and, below a schema, each value of `properties`, `$defs`, `definitions` and `dependentSchemas`; `items`, `prefixItems`, `additionalItems`, `allOf`, `anyOf` and `oneOf`; `additionalProperties` when an object; `not`, `if`, `then`, `else` and `contains`. A property named like an assertion (`format`) is not one.
  - Refused before sending, in this order: provider options (above); a blank `prompt` (`a structured call needs its prompt as text`); a `matte` that is not `#rgb`, `#rrggbb` or `#rrggbbaa` (`matte <v> is not a colour`); a `max_tokens` below 1 (`max_tokens must be at least 1`) or above 2^53 − 1; a schema whose local references are unknown (`unknown local schema reference: <ref>`), cyclic (`cyclic local schema reference: <ref>`) or expand past a million values (`local schema references expand to more than 1000000 values`), or that does not compile (`a structured call's schema is not a JSON Schema (draft 2020-12)`); a schema file that is empty, not JSON or not an object (`schema has no bytes to send`, `a structured call's schema is not a JSON file`, `a structured call's schema is a JSON object`); a context file that is empty, not UTF-8 text or a picture that does not decode (`context <i> has no bytes to send`, `context <i> is not UTF-8 text`, `context <i> is not a decodable <kind>`); a body over 200 MiB (`request body exceeds 200 MiB`).
  - The response format's `json_schema.name` is the schema's `title` with every character but ASCII letters and digits written `_`, cut to 64 characters, else `answer`.
  - Text context files are appended to the prompt as `--- context <i> ---\n<text>`, where `<i>` counts every context file from 1, pictures included, and the text is sent as read.
  - The answer is `message.parsed`, else the JSON text of `message.content` (NaN and Infinity are refused; for a repeated member, the last value wins), unwrapped from `completionState` wrappers. Its `data` is `{"json": <value>}`.
  - The check validates the value against the request's original schema (draft 2020-12, `format` not asserted). It refuses with `<path>: <message>` for the error at the smallest instance path.
- **Agent turns.**
  - The engine's transcript maps to chat messages. Tool messages drop `name`, and their pictures are regrouped into a following user message `Pictures the tools returned.`. Each picture goes as a data URL of its own kind.
  - An assistant's `arguments` string is sent byte for byte.
  - Refused before sending: provider options (above); a call without an id (`a tool call without an id cannot be sent`); a tool message without `tool_call_id` (`a tool message without a tool_call_id cannot be sent`); a message whose role is not `user`, `assistant` or `tool` (`an agent transcript has no '<role>' messages`); a tool name that does not match `^[a-z][a-z0-9_]{0,63}$` (`tool name "<n>" must be lower_snake_case, at most 64 characters`); a blank description (`tool <name> must carry a description`); parameters that are not a schema object (`tool <name> parameters must be a JSON Schema object`); a picture that is not an `image/*` file (`messages[<i>].images[<j>] is <kind>, not a picture`, counted from 0; the kinds of every picture are judged before any file is read); a picture without bytes (`an agent picture has no bytes to send`); a `max_tokens` below 1; a body over 200 MiB. An empty id counts as none.
  - The answer is `data: {"text", "tool_calls": [{"id", "name", "arguments": <object>}]}`.
- **Music.** The prompt is the only input; `duration` stays in the call key but is not sent.
  - Refused before sending: provider options (above), a blank prompt (`a music call needs its prompt as text`) and a body over 200 MiB.
  - The response is read as SSE, or as one buffered JSON object. Audio chunks are concatenated, then strictly decoded once.
  - An empty body, an event that is not a JSON object, a stream with no events, an SSE error event, no audio, conflicting media types, or audio that is not MP3 is a structural failure.
  - The answer is file `audio`, checked against the MP3 signature.
- **Cost:** `usage.cost` ([adapters.md](../../../../spec/adapters.md) §5). For music, the last `usage` seen wins, an error event's included.
  - **A request on the user's own provider key (BYOK)**, which `usage.is_byok: true` or a non-null `usage.cost_details.upstream_inference_cost` shows: `usage.cost` is then OpenRouter's fee only, and the provider bills the inference to the user's own account. The cost is `cost` plus `upstream_inference_cost`, each rounded up ([adapters.md](../../../../spec/adapters.md) §5), so a ceiling counts what the call cost in all. A BYOK answer whose upstream cost is missing, `null` or not a cost reports `None`, and the engine charges the whole hold. Without BYOK, OpenRouter reports the upstream cost as `0` or `null`, and the cost is `cost`.
- **Statuses (overrides of [adapters.md](../../../../spec/adapters.md) §3.3):**
  - **408:** `Failed { cost: None, retryable: true }`, with or without an envelope. OpenRouter's 408 means the request timed out there, after it arrived, so the provider may have done the work: the attempt is billed, and a new attempt may follow.
  - **4xx other than 408 and 429, with an OpenRouter error envelope (`{"error": {...}}`):** `Failed { cost: Some(0), retryable: false }`. OpenRouter documents that an error status generates nothing and is not charged. Without an envelope, the cost is `None`.
  - **503 with an error envelope:** `NotReceived`. No provider meets the routing requirements yet. Without an envelope, a 503 follows [adapters.md](../../../../spec/adapters.md) §3.3.
- **Reasons:** the labels are `OpenRouter image generation`, `OpenRouter structured generation`, `OpenRouter tool loop` and `OpenRouter music generation`, each with the safe detail of the error envelope, which unwraps `error.metadata.raw`.
