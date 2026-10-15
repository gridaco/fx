# fal

The reference for FX's adapters on `fal`, under the adapter contract, [spec/adapters.md](../../../../spec/adapters.md), whose sections it cites; it is normative for these adapters. [spec/providers.md](../../../../spec/providers.md) §1, §2 and §3 hold their key and endpoint, the built-in adapters and the routes FX ships.

| Capability | Contract `adapter` | `adapter_behavior` |
|---|---|---|
| `image.generate`, `image.edit` | `fal-image` | `"1"` |
| `image.generate`, `image.edit` | `fx-fal-image-v1` | `"1"` |
| `video.generate` | `fal-queue` | `1` |
| `background.remove` | `fal-run-birefnet` | `1` |

A route whose contract names an adapter of its capability with another `adapter_behavior` than the one above is refused ([adapters.md](../../../../spec/adapters.md) §4 step 1), and one that names an `adapter` not above is served by none ([adapters.md](../../../../spec/adapters.md) §1). Up to FX 0.6.1, these adapters checked only `adapter`. A video job already submitted is collected whatever `adapter_behavior` its route names, since the call's key fixed the contract: `collect` checks the capability and `adapter` alone, so a job submitted under 0.6.1 is not stranded.

- **Capabilities:**
  - `image.generate` and `image.edit` on any model (`POST {run}/<model>`, the route's model being fal's endpoint id as fal publishes it, such as `acme/img-a` and `acme/img-a/edit` (made up)). Contract `adapter`: `fal-image`, behaviour `"1"`, the string: the default of both capabilities, which serves a route whose contract names no adapter ([providers.md](../../../../spec/providers.md) §2).
  - `image.generate` (`POST {run}/<model>/text-to-image`) and `image.edit` (`POST {run}/<model>/edit`) on GPT Image. Contract `adapter`: `fx-fal-image-v1`, behaviour `"1"`, the string: only a route that names it, as the built-in routes do.
  - `video.generate`, a long job on the queue: `POST {queue}/<model>`. Contract `adapter`: `fal-queue`, behaviour `1`.
  - `background.remove` (`POST {run}/<model>`). Contract `adapter`: `fal-run-birefnet`, behaviour `1`.
- **Credential:** `authorization: Key <FAL_KEY>`, on the run and queue hosts only.
- **Provider options** ([adapters.md](../../../../spec/adapters.md) §8): the parameters of `provider_options.fal` are added at the end of each body (a video's submit body), after FX's members, and are refused, as the first of the values below, when they set a member FX owns: every member of that body (below), and `sync_mode`, which changes how fal returns its output. So images on any model own `prompt`, `image_urls`, `num_images` (which would change how many pictures the answer holds) and `sync_mode`; images on GPT Image `prompt`, `num_images`, `image_size`, `quality`, `background`, `output_format`, `image_urls`, `mask_url` and `sync_mode`; video `prompt`, `image_url`, `end_image_url`, `aspect_ratio`, `resolution`, `duration` and `sync_mode`; background removal all eight members of its body.
- **Images on any model** (`fal-image`). The adapter sends what fal's image endpoints share, and nothing FX chose for one model: no size, no quality, no output format. A parameter of the model's own goes in `provider_options.fal`.
  - Refused before sending, in this order:
    - provider options (above);
    - a blank `prompt`: `an image call needs its prompt as text`;
    - a mask: `fal's generic image adapter sends no mask: give the model's own parameter in provider_options.fal`;
    - `background: transparent`: `fal's generic image adapter sends no transparent background: give the model's own parameter in provider_options.fal`; a `background` other than `auto`, `opaque` or `transparent`: `background must be auto, opaque or transparent`;
    - a `size` that is not `auto` or `<W>x<H>`: `size must be auto or WIDTHxHEIGHT`; an exact size: `fal's generic image adapter sends no exact size: give the model's own parameter in provider_options.fal`;
    - references on a generate: `fal image generation takes no references` (fal's text-to-image endpoints take no pictures; an edit's endpoint takes them);
    - more input pictures than the route's `max_input_pictures`, when it declares one: `<route> takes at most <n> input pictures, not <count>`;
    - then, in the order `image`, `references[<i>]` (counted from 0): a picture that is not an `image/*` file: `<member> is <kind>, not a picture`.
  - The body is `{prompt, image_urls?}`, then the provider options. `image_urls` is an edit's `image`, then its `references`, as data URLs under their own kind. `size: auto`, `background: auto` and `background: opaque` are not sent: the model draws its own default, and the check holds an opaque request to an opaque picture.
  - The answer is read as GPT Image's (below): `root.images`, which must hold exactly one object (else `fal image generation returned no single image`, `Failed { cost, retryable: false }`: FX asks for no count, so another attempt would draw as many), a data URL or a `fal.media` download, of a declared PNG, JPEG or WebP. A PNG is kept as it is; a JPEG or WebP is decoded and encoded as PNG, the same pixels (`fal's image is not decodable`, `fal's image could not be encoded as PNG`, each `Failed { cost, retryable: true }`). The answer is file `image` (`image/png`), with `data: null`. The deadline is 600 s for the POST and the download together.
  - The check is every image route's ([capabilities.md](../../../../spec/capabilities.md) §2): a PNG that decodes, opaque when asked for.
  - Facts: [Grida Models](https://github.com/gridaco/models) `v1.20261014.1` records 32 image endpoints on fal. Every one of the 29 that record parameters takes `prompt` (Reve 2.1's three record none), and 13 of the 14 that take pictures take them as `image_urls`, a list; Ideogram 4.5's edit takes one `image_url` and `reference_image_urls`, a shape this adapter does not send, so it needs an adapter of the project's own. fal's published schemas of its image endpoints (checked 2026-10-14) answer `images`, a list of files each with a `url` and an optional `content_type`, `width` and `height`; where an endpoint takes `num_images`, it defaults to 1, and `sync_mode` to false. No other member is common to them: `num_images` is on 24 of the 32, `output_format` on 24 (`jpeg` by default on some), `image_size` on 19, and a mask (`mask_url`) on one, Ideogram 4.5's edit.
- **Images on GPT Image** (`fx-fal-image-v1`).
  - Refused before sending, in this order:
    - provider options (above);
    - a blank `prompt` (`an image call needs its prompt as text`), or one over 32 000 characters (`the prompt is longer than 32000 characters`);
    - a bad `background`: `background must be auto, opaque or transparent`;
    - references on a generate: `fal image generation takes no references`;
    - more than 16 pictures: `fal image edits support at most 16 input references`;
    - a `size` that is not `auto` or `WxH` (`fal image size must be auto or WIDTHxHEIGHT`), or outside the envelope of [OpenAI's](../openai/README.md), with sentences starting `fal image size`;
    - then, in the order `image`, `references[<i>]` (counted from 0), `mask`: a picture that is not an `image/*` file: `<member> is <kind>, not a picture`.
  - The body is `{prompt, num_images: 1, image_size?, quality: max, background, output_format: png, image_urls?, mask_url?}`, then the provider options. `image_size` is `"auto"` or `{width, height}`, and pictures are data URLs under their own kind.
  - The answer is `root.images[0]`, where `root` is the payload's `data` when that is an object, else the payload. It is either a data URL, or a URL on fal's declared download hosts, `fal.media` or a subdomain of it (https, port 443, no userinfo, no fragment; [providers.md](../../../../spec/providers.md) §1) downloaded once, at the URL exactly as fal gave it, with `accept: image/*`. Any other URL is refused before a request. The download is at most 64 MiB.
  - The deadline is 600 s for the POST and the download together.
- **Video.**
  - Refused before sending, in this order:
    - provider options (above);
    - a blank prompt (`a clip needs its prompt as text`), or one over 20 000 characters (`the prompt is longer than 20000 characters`);
    - no `first_frame`, on a route that does not declare the feature `text_to_video` (the built-in route does not): `this route draws from a first frame`;
    - a `last_frame` without a `first_frame`: `last_frame needs first_frame`;
    - a `duration` that is not a whole number from 3 to the route's most: `this route draws whole seconds from 3 to <most>, not <duration>`. The most is the route's `price.max_units` when it is priced per second (whole seconds, rounded down), else 10;
    - a `resolution` (default `720p`) the route does not draw, or an `aspect_ratio` (default `9:16`) not `9:16` or `16:9`: `this route draws no <resolution> clip at <aspect_ratio>`. The route draws its price's tiers when it is priced `by: resolution`, else `360p`, `720p`, `1080p` and `4k`. A capped route (`price: {cap_usd: N}`) states neither tiers nor `max_units`, so it draws those four tiers, from 3 to 10 s;
    - then `first_frame` and `last_frame`, each when it is not an `image/*` file: `<member> is <kind>, not a picture`.
  - The submit body is `{prompt, image_url?, end_image_url?, aspect_ratio, resolution, duration}`, with `duration` a JSON integer, then the provider options. `image_url` is the first frame, left out of a request without one, which only a route that declares `text_to_video` sends. The minimum, the whole seconds, the aspect ratios and the defaults are this adapter's, since a route table has no member for them. A route of your own is sent this same body, so it fits only an endpoint that takes it: one that names its first frame otherwise, or takes `duration` as a string, does not.
  - The handle is `{"request_id", "status_path", "response_path"}`: the returned URLs minus `{queue}/`, kept byte for byte. A path is non-empty segments of `A-Z a-z 0-9 . _ ~ + = , @ -`, none of them `.` or `..`. It may end in `?` and a plain query: `name` or `name=value` pairs joined by `&`. A name is 1–64 of `A-Z a-z 0-9 . _ ~ -`; a value is up to 256 of those plus `+ , : @`. A name holding `auth`, `credential`, `expires`, `key`, `password`, `policy`, `secret`, `session`, `sig` or `token`, or starting `x-amz-` or `x-goog-`, is not kept. The whole reference is at most 1024 characters. `request_id` matches `^[A-Za-z0-9_.:-]{1,96}$`. A 2xx without such a `request_id` and two such URLs under the queue base is `Uncertain`. Its reason ends `(request <id>)` when the body's `request_id`, else the response's request-id header, is a safe id ([adapters.md](../../../../spec/adapters.md) §7).
  - `collect` polls `status_path` every 5 s for at most 1500 s. Each read is bounded by the smaller of the time left and 60 s. It then reads `response_path`, then downloads `video.url` once, at the URL exactly as fal gave it (https, no credential, at most 512 MiB). The bytes decide: the file is the clip when it carries the MP4 signature and an MP4 video track. A `content-type` header and the result's `content_type` are not read. The status and result reads are GETs with the credential and no `content-type`.
  - The answer is file `video` (`video/mp4`) with `data: {"facts": {"width", "height", "duration_seconds", "fps"}}`.
  - The check compares the clip's size and duration with the request, read with the defaults above and none of the route's limits ([adapters.md](../../../../spec/adapters.md) §6): a route's tiers, length and features are not part of its identity, so a job sent before its route's table changed is checked against what it asked for. A request whose `duration` is not whole seconds from 3, which only a stand-in's can be, is refused (`this route draws whole seconds from 3, not <duration>`). The size has a short side of 360, 720, 1080 or 2160 for `360p`, `720p`, `1080p` and `4k`, oriented by the aspect ratio; a clip of another tier the route draws is not held to a size. The duration must be within `1/fps + 0.01` s of the request.
- **Background removal.** Refused before sending: provider options (above); an `image` that is not an `image/*` file: `image is <kind>, not a picture`. The body is `{image_url, model: "General Use (Light)", operating_resolution: "1024x1024", output_mask: false, refine_foreground: true, output_format: png, mask_only: false, sync_mode: true}`, then the provider options. The answer is `root.image`: a data URL, or a `fal.media` URL as for images, downloaded with `accept: image/*` (a URL on any other host is never fetched, and the send fails as [adapters.md](../../../../spec/adapters.md) §3.4 says). It is file `image`, checked as a PNG. The deadline is 300 s.
- **Cost:** `usage.cost` at the payload's top level ([adapters.md](../../../../spec/adapters.md) §5), else `None`.
- **Statuses on the run host (overrides of [adapters.md](../../../../spec/adapters.md) §3.3):**
  - **4xx other than 408 and 429:** `Failed { cost: Some(0), retryable: false }`. fal validates a request before running it, and does not bill a refusal.
- **Statuses of the video submit:**
  - **4xx other than 408 and 429:** `Failed { cost: Some(0), retryable: false }`.
  - **500, 502 and 503:** `NotReceived`. fal answers these without taking the job.
  - **504 and other 5xx:** `Uncertain`.
- **Collect statuses:**
  - A status read of 404, 410 or another 4xx (not 401, 403, 408 or 429), or a status with a truthy `error`, is `Ended`. Its reason gives `error_type` and the error text, each redacted before it is cut (to 100 and 500 characters).
  - A 408, a 429, a 5xx, a transport failure or a body that is not a JSON object is a failed status or result read, and polling goes on.
  - A 401, 403 or 3xx on a status or result read is `Unreachable`, and so is a status that names another `request_id`.
  - The download is made once per collect ([adapters.md](../../../../spec/adapters.md) §7). A transport failure, a refusal by the transport, a 3xx, 401, 403, 408, 429, a 5xx or another status outside 2xx and 4xx is `Unreachable`; a later collect reads the result and downloads again. Another 4xx is `Ended`, and so is a body that is empty, over the cap or not MP4 by its bytes, or a clip with no video stream.
  - A handle that is not one the adapter wrote (`a fal video job handle is not one this adapter wrote`) is `Unreachable`, and nothing is requested.
  - A fal job that may still be running is never settled.
  - An `Ended` after the result read (a result without an https video, or a download that ends the job) comes from a job fal finished, so it costs that read's `usage.cost`, as an answer would ([adapters.md](../../../../spec/adapters.md) §6). Every other `Ended` costs `None`, and so does a result read without a cost: the run that submitted the job then charges its whole hold.
- **Reasons:** the labels are `fal image generation`, `fal output image download`, `fal video submission`, `fal video job status`, `fal video job result`, `fal output video download` and `fal background removal`. Response bodies are never quoted, except the bounded `error_type` and error of a failed job.
