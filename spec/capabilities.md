# Capabilities

A capability is a kind of paid call: `image.generate`, `agent.turn`, `mesh.rig`, and so on. This document defines, for each capability FX ships:
- its canonical request: the members, their types, and which are files;
- what an answer returns: files by name, with kinds, and `data`;
- the features and limits a route may declare.

How a provider's adapter turns a request into an exchange, and how it is billed, is [providers.md](providers.md). How the call is keyed is [identity.md](identity.md) §9.

## 1. Requests and answers

**Requests.**
- A request is a JSON object: the **canonical request** ([identity.md](identity.md) §9). Files are file values (`{"file": "<digest>"}`, [protocol.md](protocol.md) §3.2).
- A body's `ctx` call sends whatever members it gives. A paid built-in sends its params, without `vars`, then its input ports (§14). The member tables below list the members in that order; `agent.turn`, which no built-in sends, in the order of [protocol.md](protocol.md) §6.2.
- A paid built-in leaves out a param the step did not give and that has no default, and an input port without a value: neither is a member of the request. A param whose value is missing or `null` goes as `null` (§14; [protocol.md](protocol.md) §5.3 `params`). Absent and `null` are different requests, and so different call keys ([identity.md](identity.md) §9).
- Every adapter MUST check a request against its capability right after its route's contract, before anything else ([providers.md](providers.md) §5), with these sentences:
  - A request that is not an object: `a request for <capability> is a JSON object`.
  - A member the capability does not define is refused: `<capability> takes no member <name>`.
  - Then each member, in the table's order:
    - A member whose value is `null` is absent. It stays in the call key, but no adapter reads it.
    - A required member that is absent: `<capability> needs <name>`.
    - A member of the wrong type: `<name> is <type>`. The type words are `text`, `a number`, `a whole number`, `true or false`, `an object`, `a list`, `a file`, `a list of files`, `files by name`, and `a JSON file or object`.
- Checks on a member's value beyond its type are the route's, in [providers.md](providers.md) §9: non-blank text, ranges, enumerations, sizes, counts.

In the tables below, a member's type is one of:
- `text`, `number`, `integer` (a number with no fractional part), `boolean`, `object` or `list`;
- `file` (a file value);
- `file[]` (a list of file values);
- `file{}` (an object of file values by name);
- `file | object` (a `json` file, or the JSON object inline).

**Answers.**
- An answer holds files by name, which the engine stores ([store.md](store.md) §3), and `data`: JSON, or `null`. It holds what the sections below name and nothing else.
- A paid built-in maps an answer onto its outputs this way:
  - each output port takes the file of its name, else the first file whose kind's family matches the port's;
  - a declared `json` output takes `data.json`;
  - each member of `data.facts` becomes a node fact, and a string `data.verdict` becomes the fact `verdict`.

**Features.**
- A route declares the features it supports, in its table entry ([identity.md](identity.md) §7). A step's `requires:` names features of the capabilities its type calls. A name that is a feature of none of them is refused while planning: `<name> is not a(n) <capability>[ or <capability>] feature (spec/capabilities.md)`, followed by `; FX calls it <feature>` for a name FX's predecessor used ([providers.md](providers.md) §11). A capability without a section here (§13, or a project's own) is not checked this way. A known feature the step's route lacks is refused as `<route> does not support <features>`.
- Each capability's section lists its features. Only those names have a meaning for it. A route in the built-in table ([providers.md](providers.md) §10) MUST declare only names from its capability's list, and only what its adapter honours.
- A few image features describe the provider's model and change nothing FX sends, whatever `jpeg_output`, `webp_output` or `hosted_url_reference_input` say. Every route FX ships sends pictures inline. A route whose provider takes an output format (OpenAI, fal) asks for PNG; OpenRouter's image endpoint takes none, so its routes ask for nothing. Every route's check refuses an answer that is not a PNG (§2).

## 2. `image.generate`

A picture from a prompt.

| Member | Type | | |
|---|---|---|---|
| `prompt` | text | required | sent verbatim; the rendered template of a built-in |
| `size` | text | optional | `auto`, or `<W>x<H>` in pixels: an exact size the answer must have |
| `background` | text | optional | `auto` (the default), `opaque` or `transparent` |
| `references` | file[] | optional | input pictures; a route without `image_input` refuses a non-empty list |

**Answer:** the file `image` (`image/png`), with `data: null`.

**Every route's check:**
1. the file is a PNG, by its kind and signature: `the answer is <kind>, not image/png`;
2. it decodes: `the image data is not decodable`;
3. an exact `size` matches: `the image is <w>x<h>, not <W>x<H>`;
4. `transparent`: the image has an alpha channel (the `has_alpha` fact): `the picture asked for as transparent has no alpha channel`;
5. `opaque`: every pixel is opaque (the `opaque` fact): `the picture asked for as opaque has transparent pixels`.

**Features:**

| Feature | Meaning |
|---|---|
| `text_to_image` | the route draws from the prompt alone |
| `alpha` | `background: transparent` is honoured: the answer has an alpha channel |
| `opaque_background` | `background: opaque` is honoured |
| `auto_background` | `background: auto` is honoured |
| `exact_size` | a `size` of `<W>x<H>` is honoured exactly |
| `custom_exact_size` | any `<W>x<H>` within the route's envelope is served, not only a fixed list of sizes |
| `flexible_size` | `size` may be absent or `auto` |
| `image_input` | the route takes input pictures: `references` |
| `hosted_url_reference_input` | the provider can also fetch input pictures by URL; FX always sends them inline |
| `png_output` | the model can encode PNG: every route FX ships needs a PNG answer (check 1), and asks for one where its provider takes an output format |
| `jpeg_output` | the model can encode JPEG |
| `webp_output` | the model can encode WebP |
| `maximum_quality` | the route asks for the model's highest quality |
| `authored_prompt_passthrough` | the prompt is sent verbatim, never rewritten |

**Limits a route may have:** an exact-size envelope or a fixed list of sizes, no transparency, and no references. They are enforced as refusals ([providers.md](providers.md) §9). No route FX ships takes references for `image.generate`, so none declares `image_input` for it.

## 3. `image.edit`

A picture made from input pictures, optionally within a mask.

| Member | Type | | |
|---|---|---|---|
| `prompt` | text | required | |
| `size` | text | optional | as for `image.generate` |
| `background` | text | optional | as for `image.generate` |
| `image` | file | required | the first input picture |
| `mask` | file | optional | which part of `image` may change: a PNG the size of `image` whose fully transparent pixels (alpha 0) mark the editable region. This is GPT Image's convention, which every route FX ships that takes a mask follows; adapters pass the mask through unchanged. It is a strong hint, not a protected region |
| `references` | file[] | optional | more input pictures, after `image` |

**Answer and check:** as for `image.generate`: the file `image` (`image/png`), with `data: null`, and every route's check of §2.

**Features:**

| Feature | Meaning |
|---|---|
| `alpha` | as for `image.generate` |
| `opaque_background` | as for `image.generate` |
| `auto_background` | as for `image.generate` |
| `exact_size` | as for `image.generate` |
| `custom_exact_size` | as for `image.generate` |
| `flexible_size` | as for `image.generate` |
| `image_input` | the route takes input pictures: `image`, then `references` |
| `hosted_url_reference_input` | as for `image.generate` |
| `png_output` | as for `image.generate` |
| `jpeg_output` | as for `image.generate` |
| `webp_output` | as for `image.generate` |
| `maximum_quality` | as for `image.generate` |
| `authored_prompt_passthrough` | as for `image.generate` |
| `mask` | the route takes a `mask` |

**Limits a route may have:** an exact-size envelope or a fixed list of sizes, no transparency, no mask, and a limit on the number of input pictures. They are enforced as refusals ([providers.md](providers.md) §9). Every route FX ships takes at most 16 input pictures (`image` and `references`; the mask does not count).

## 4. `structured.generate`

A JSON value that meets a schema.

| Member | Type | | |
|---|---|---|---|
| `prompt` | text | required | |
| `system` | text | optional | a system prompt; blank means none |
| `matte` | text | optional | the colour pictures are flattened on when a route reduces them; `#ffffff` by default |
| `max_tokens` | integer | optional | at least 1; 16 000 by default |
| `schema` | file \| object | required | a JSON Schema (draft 2020-12) whose root is an object |
| `context` | file[] | optional | pictures (`image/*`) are shown; any other file is read as UTF-8 text and appended to the prompt |

**Answer:** no files. `data` is `{"json": <value>}`.

**Every route's check:** the value meets `schema`, as given, not as a route reshapes it for its provider. The refusal is `<where>: <message>`, for the error at the smallest instance location.

**Features:**

| Feature | Meaning |
|---|---|
| `structured_output` | the provider is held to the schema while it answers |
| `image_input` | pictures in `context` are shown to the model |

## 5. `agent.turn`

One turn of the engine's agent loop ([protocol.md](protocol.md) §6.2). The engine builds every request; a body never sends one directly.

| Member | Type | | |
|---|---|---|---|
| `system` | text | required | |
| `messages` | list | required | the transcript, windowed: `{"role": "user", "content", "images"?}`, `{"role": "assistant", "content", "tool_calls": [{"id", "name", "arguments": <canonical JSON text>}]}`, `{"role": "tool", "name", "tool_call_id"?, "content", "images"?}`; `images` are file values |
| `tools` | list | required | `[{"name", "description", "parameters": <JSON Schema>}]`, the engine's `submit` included |
| `tool_choice` | text | required | `auto` or `required` |
| `max_tokens` | integer | optional | at least 1 |

**Answer:** no files. `data` is `{"text": <text>, "tool_calls": [{"id": <text>, "name": <text>, "arguments": <object>}]}`. The engine checks this shape ([protocol.md](protocol.md) §6.2).

**Features:**

| Feature | Meaning |
|---|---|
| `tool_use` | the model calls the request's tools |
| `image_input` | pictures in the transcript are shown to the model |

## 6. `video.generate`

A clip from a prompt and frames. A long job.

| Member | Type | | |
|---|---|---|---|
| `prompt` | text | required | |
| `duration` | number | optional | seconds; a route may take whole seconds only |
| `resolution` | text | optional | a route's tier name, such as `720p` |
| `aspect_ratio` | text | optional | such as `9:16` |
| `first_frame` | file | optional | the opening frame; a route may require it |
| `last_frame` | file | optional | the closing frame; needs `first_frame` |

**Answer:** the file `video` (`video/mp4`). `data` is `{"facts": {"width", "height", "duration_seconds", "fps"}}`, with numbers from the clip's file facts ([facts.md](facts.md)).

**A route's check:** the clip's size and duration against the request ([providers.md](providers.md) §9.3).

**Features:**

| Feature | Meaning |
|---|---|
| `first_last_frame` | both `first_frame` and `last_frame` are honoured |

**Pricing:** per second, in tiers by `resolution` ([identity.md](identity.md) §12).

## 7. `mesh.generate`

A 3D model from views of one subject. A long job.

| Member | Type | | |
|---|---|---|---|
| `face_limit` | integer | optional | the most faces |
| `quad` | boolean | optional | quad topology |
| `texture` | boolean | optional | a textured model |
| `pbr` | boolean | optional | PBR materials |
| `views` | file{} | required | pictures by view name; the multiview route takes `front` (required), `back`, `left`, `right`, each PNG or JPEG |

**Answer:** the file `model`, whose kind is `model/fbx` or `model/gltf-binary`, taken from its bytes. `data` is `{"facts": {"model_kind": <kind>}}`.

**Features:**

| Feature | Meaning |
|---|---|
| `multiview` | the route takes several named views of the subject |
| `textured_mesh` | `texture: true` is honoured |
| `quad` | `quad: true` is honoured |
| `pbr` | `pbr: true` is honoured |

## 8. `mesh.rig`

A rigged model from an unrigged one. A long job.

| Member | Type | | |
|---|---|---|---|
| `rig_type` | text | optional | `biped` by default |
| `skeleton` | text | optional | the skeleton convention; `mixamo` by default |
| `allow_negative_check` | boolean | optional | rig a model the provider's free pre-check doubts; false by default |
| `model` | file | required | `model/gltf-binary` |

**Answer:** the file `model` (`model/gltf-binary`). `data` is `{"facts": {"riggable": <boolean or null>, "checked_rig_type": <the check's answer or null>, "advisory_override": <boolean>}}`.

**Features:**

| Feature | Meaning |
|---|---|
| `biped` | `rig_type: biped` is served |
| `glb_input` | the route takes a `model/gltf-binary` model |
| `rig_check` | a free pre-check runs before the paid rig |

## 9. `music.generate`

A piece of music.

| Member | Type | | |
|---|---|---|---|
| `prompt` | text | required | structure and length are asked for in the prompt |
| `duration` | number | optional | seconds; part of the call key and of pricing, but no route FX ships sends it |

**Answer:** the file `audio` (`audio/mpeg`), with `data: null`. **Check:** the MP3 signature.

**Features:** none.

## 10. `sound.generate`

A sound effect.

| Member | Type | | |
|---|---|---|---|
| `prompt` | text | required | |
| `duration` | number | optional | seconds; the model chooses when absent |
| `prompt_influence` | number | optional | 0 to 1 |
| `loop` | boolean | optional | a seamless loop; false by default |

**Answer:** the file `audio` (`audio/mpeg`), with `data: null`. **Check:** not empty, MP3, the MP3 signature.

**Features:**

| Feature | Meaning |
|---|---|
| `exact_duration` | an explicit `duration` is honoured |

## 11. `speech.generate`

Spoken text.

| Member | Type | | |
|---|---|---|---|
| `text` | text | required | sent verbatim, delivery tags such as `[excited]` included |
| `voice` | text | required | the provider's voice id |
| `stability` | number | optional | 0 to 1 |
| `language_code` | text | optional | an ISO language code; `""` counts as absent |
| `max_chars` | integer | optional | the most characters, for pricing per thousand characters; never sent |

**Answer:** the file `audio` (`audio/mpeg`), with `data: null`. **Check:** as for `sound.generate`.

**Features:**

| Feature | Meaning |
|---|---|
| `audio_tags` | bracketed delivery tags are performed, not spoken |
| `stability` | `stability` is honoured |

## 12. `background.remove`

A picture's subject on a transparent background.

| Member | Type | | |
|---|---|---|---|
| `image` | file | required | |

**Answer:** the file `image` (`image/png`), with `data: null`. **Check:** a PNG by kind and signature.

**Features:** none.

FX ships an adapter for it on fal, but the built-in route table has no route for it yet ([providers.md](providers.md) §10).

## 13. Capabilities without an adapter

`vision.annotate`, `vision.review` and `structured.review` are declared by the standard library's annotator and judges, but FX ships no adapter for them. A call on one is `no_route`.

## 14. The standard library

The paid built-ins (`fx/<capability>@1`) send the requests above:
- every param except `vars`, then every input port, in the order the member tables list them;
- each name a member of its capability, with a type the member admits;
- not a param the step left out and that has no default, and not an input port without a value: these are absent from the request;
- a param whose value is missing or `null` as `null`, which an adapter reads as absent (§1).

A member is required exactly when the built-in requires it: an input port without `?`, or a param with no `default` that is not `x-fx-optional`. Each output port of a built-in names a file of its capability's answer, or is the `json` output that takes `data.json`.

The engine's tests hold the built-in catalog, and the engine's own table of capabilities, to this document.
