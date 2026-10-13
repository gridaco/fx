# OpenAI

The reference for FX's adapters on `openai`, under the adapter contract, [spec/adapters.md](../../../../spec/adapters.md), whose sections it cites; it is normative for these adapters. [spec/providers.md](../../../../spec/providers.md) §1, §2 and §3 hold their key and endpoint, the built-in adapters and the routes FX ships.

| Capability | Contract `adapter` | `adapter_behavior` |
|---|---|---|
| `image.generate`, `image.edit` | `fx-openai-image-v1` | `"1"` |

- **Capabilities:** `image.generate` (`POST {base}/images/generations`, JSON) and `image.edit` (`POST {base}/images/edits`, multipart). One request per send. Contract `adapter`: `fx-openai-image-v1`, behaviour `"1"`, the string: the number `1` is another contract, with another fingerprint, and is refused ([adapters.md](../../../../spec/adapters.md) §4 step 1), as OpenRouter's image adapter refuses the number `3`.
- **Credential:** `authorization: Bearer <OPENAI_API_KEY>`.
- **Limits:** deadline 600 s per send; response cap 64 MiB.
- **Wire.** Always `n: 1`, `output_format: png`, `quality: max`, `moderation: low`, and `background` (including `auto`). `size` only when given (including `auto`). Never `input_fidelity` (the model refuses it), `response_format`, `seed` or `stream`.
  - An edit sends its text fields first, in that order.
  - Then each input image (`image` first, then `references`) as part `image[]`, named `reference-NN.<png|jpg|webp>` with `NN` from `01`.
  - Then the mask as part `mask`, named `reference-00.<ext>`.
- **Refusals ([adapters.md](../../../../spec/adapters.md) §4 step 4–5), in this order:**
  - a blank `prompt`: `an image call needs its prompt as text`;
  - a `background` other than `auto`, `opaque` or `transparent`: `background must be auto, opaque or transparent`;
  - a `size` that is not `auto` or `WxH`: `OpenAI image size must be auto or WIDTHxHEIGHT`; or one that leaves the envelope, checked in this order: edges multiples of 16 (`OpenAI image size edges must be multiples of 16`), the longer edge at most 3840 (`OpenAI image size edges must not exceed 3840 pixels`), aspect ratio at most 3:1 (`OpenAI image size aspect ratio must not exceed 3:1`), and 655 360 to 8 294 400 pixels (`OpenAI image size must contain between 655360 and 8294400 pixels`);
  - references on a generate (a non-empty `references`): `OpenAI image generation takes no references`;
  - more than 16 input images: `OpenAI image edits support at most 16 input images`;
  - an input or mask kind other than PNG, JPEG or WebP: `<member> is <kind>; OpenAI takes PNG, JPEG or WebP`; or bytes that do not decode as their kind: `<member> is not a decodable <kind>`. `<member>` is `image`, `references[<i>]` (counted from 0) or `mask`.
- **Answer.** `data[0].b64_json` is the one image. It must be strict base64, else the send is a structural failure; a `url` instead is a structural failure too, never fetched. Whether the bytes are a PNG is the check's judgement ([capabilities.md](../../../../spec/capabilities.md) §2, check 1). The answer is file `image` (`image/png`) and `data: null`. Cost: `None`, since OpenAI reports token usage but no cost.
- **Check:** the image checks of [capabilities.md](../../../../spec/capabilities.md) §2.
- **Statuses (overrides of [adapters.md](../../../../spec/adapters.md) §3.3):**
  - **408:** `Failed { cost: None, retryable: true }`. OpenAI's 408 is its own timeout, after the request arrived, so the image may have been drawn: the attempt is billed, and a new attempt may follow.
  - **4xx other than 408 and 429:** `Failed { cost: Some(0), retryable: false }`. OpenAI does not bill a request it rejects.
  - **429 with `error.code` or `error.type` `insufficient_quota`:** `Failed { cost: Some(0), retryable: false }`. The quota is exhausted, so the call ends.
  - **Any other 429:** `NotReceived` ([adapters.md](../../../../spec/adapters.md) §3.3); a `retry-after` that is a plain number of seconds is named in the reason (`; retry-after <n>`).
  - **400 whose `error.type` or `error.code` (ignoring case) is `moderation_blocked`, or contains `safety` or `content_policy`:** `Failed { cost: None, retryable: false }`. Moderation can run after generation, so the cost is unknown.
- **Reasons:** the label is `OpenAI image generation`, with the safe detail of the error envelope.
