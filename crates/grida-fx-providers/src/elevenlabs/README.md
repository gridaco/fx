# ElevenLabs

The reference for FX's adapters on `elevenlabs`, under the adapter contract, [spec/adapters.md](../../../../spec/adapters.md), whose sections it cites; it is normative for these adapters. [spec/providers.md](../../../../spec/providers.md) §1, §2 and §3 hold their key and endpoint, the built-in adapters and the routes FX ships.

| Capability | Contract `adapter` | `adapter_behavior` |
|---|---|---|
| `sound.generate` | `elevenlabs-sound-effect` | `1` |
| `speech.generate` | `elevenlabs-speech` | `1` |

- **Capabilities:**
  - `sound.generate`: `POST {base}/sound-generation?output_format=mp3_44100_192`. Contract `adapter`: `elevenlabs-sound-effect`, behaviour `1`.
  - `speech.generate`: `POST {base}/text-to-speech/<voice>?output_format=mp3_44100_192`. Contract `adapter`: `elevenlabs-speech`, behaviour `1`.
- **Credential:** `xi-api-key: <ELEVENLABS_API_KEY>`, plus headers `content-type: application/json` and `accept: audio/mpeg`.
- **Limits:** deadline 120 s; response cap 64 MiB.
- **Provider options** ([adapters.md](../../../../spec/adapters.md) §8): the parameters of `provider_options.elevenlabs` are added at the end of the body, after FX's members, and are refused, as the first of the values below, when they set a member FX owns: every member of the body (below), and `output_format`, which FX sets in the query. So sound owns `text`, `model_id`, `loop`, `duration_seconds`, `prompt_influence` and `output_format`; speech `text`, `model_id`, `voice_settings`, `language_code` and `output_format`. `voice_settings` is FX's whole: provider options cannot add a setting to it.
- **Sound.**
  - Refused before sending, in this order: provider options (above); a blank prompt (`a sound call needs its prompt as text`), or one over 450 characters (`sound effect prompt must be at most 450 characters`); a `duration` outside 0.5 to 30 (`duration must be between 0.5 and 30`); a `prompt_influence` outside 0 to 1 (`prompt_influence must be between 0 and 1`).
  - The body is `{text, model_id: <route model>, loop, duration_seconds?, prompt_influence?}`, with `loop` false when absent, then the provider options.
- **Speech.**
  - Refused before sending, in this order: provider options (above); blank text (`a speech call needs its text`); a blank voice (`a speech call needs its provider voice`); a voice that does not match `^[A-Za-z0-9_-]{1,128}$` (`the provider voice is not a voice id`); text over 5000 characters (`speech text must be at most 5000 characters`); a `stability` outside 0 to 1 (`stability must be between 0 and 1`); a `language_code` that is blank but not `""` (`language_code must not be blank`).
  - The body is `{text, model_id, voice_settings?: {stability}, language_code?}`, then the provider options. `language_code` `null` or `""` is not sent. `max_chars` is for pricing only and is never sent.
- **Answer:** the response body is the audio, as file `audio`. Its kind is the normalized `content-type`, or `audio/mpeg` when that header is absent. A `content-type` outside the audio family is kept as its lowercased base type, or as `application/octet-stream` when it is not a media type, so the check refuses it. `data: null`. Cost: `None`, since ElevenLabs reports characters, not dollars.
- **Check, in order:**
  - not empty: `<label> returned no audio data`;
  - an MP3: `requested mp3 but received <kind>`;
  - the MP3 signature: `audio bytes do not match declared media type audio/mpeg`.
- **Statuses:** [adapters.md](../../../../spec/adapters.md) §3.3 as written: a 408 is `NotReceived` (ElevenLabs did not read the whole request), and a 4xx other than 408 and 429 costs `None`.
- **Reasons:** the labels are `ElevenLabs sound generation` and `ElevenLabs speech generation`. The error body's `detail.message` is never quoted.
