# fal

The reference for FX's adapters on `fal`, under the adapter contract, [spec/adapters.md](../../../../spec/adapters.md), whose sections it cites; it is normative for these adapters. [spec/providers.md](../../../../spec/providers.md) §1, §2 and §3 hold their key and endpoint, the built-in adapters and the routes FX ships.

| Capability | Contract `adapter` | `adapter_behavior` |
|---|---|---|
| `image.generate`, `image.edit` | `fx-fal-image-v1` | `"1"` |
| `video.generate` | `fal-queue` | `1` |
| `background.remove` | `fal-run-birefnet` | `1` |

A route whose contract names another `adapter` or `adapter_behavior` than the ones above is refused ([adapters.md](../../../../spec/adapters.md) §4 step 1). Up to FX 0.6.1, these adapters checked only `adapter`. A video job already submitted is collected whatever `adapter_behavior` its route names, since the call's key fixed the contract: `collect` checks the capability and `adapter` alone, so a job submitted under 0.6.1 is not stranded.

- **Capabilities:**
  - `image.generate` (`POST {run}/<model>/text-to-image`) and `image.edit` (`POST {run}/<model>/edit`). Contract `adapter`: `fx-fal-image-v1`, behaviour `"1"`, the string.
  - `video.generate`, a long job on the queue: `POST {queue}/<model>`. Contract `adapter`: `fal-queue`, behaviour `1`.
  - `background.remove` (`POST {run}/<model>`). Contract `adapter`: `fal-run-birefnet`, behaviour `1`.
- **Credential:** `authorization: Key <FAL_KEY>`, on the run and queue hosts only.
- **Images.**
  - Refused before sending, in this order:
    - a blank `prompt` (`an image call needs its prompt as text`), or one over 32 000 characters (`the prompt is longer than 32000 characters`);
    - a bad `background`: `background must be auto, opaque or transparent`;
    - references on a generate: `fal image generation takes no references`;
    - more than 16 pictures: `fal image edits support at most 16 input references`;
    - a `size` that is not `auto` or `WxH` (`fal image size must be auto or WIDTHxHEIGHT`), or outside the envelope of [OpenAI's](../openai/README.md), with sentences starting `fal image size`;
    - then, in the order `image`, `references[<i>]` (counted from 0), `mask`: a picture that is not an `image/*` file: `<member> is <kind>, not a picture`.
  - The body is `{prompt, num_images: 1, image_size?, quality: max, background, output_format: png, image_urls?, mask_url?}`. `image_size` is `"auto"` or `{width, height}`, and pictures are data URLs under their own kind.
  - The answer is `root.images[0]`, where `root` is the payload's `data` when that is an object, else the payload. It is either a data URL, or a URL on `fal.media` or a subdomain of it (https, port 443, no userinfo, no fragment) downloaded once, at the URL exactly as fal gave it, with `accept: image/*`. Any other URL is refused before a request. The download is at most 64 MiB.
  - The deadline is 600 s for the POST and the download together.
- **Video.**
  - Refused before sending, in this order:
    - a blank prompt (`a clip needs its prompt as text`), or one over 20 000 characters (`the prompt is longer than 20000 characters`);
    - no `first_frame`: `this route draws from a first frame`;
    - a `duration` that is not a whole number from 3 to 10: `this route draws whole seconds from 3 to 10, not <duration>`;
    - a `resolution` (default `720p`) not among `360p`, `720p`, `1080p`, `4k`, or an `aspect_ratio` (default `9:16`) not `9:16` or `16:9`: `this route draws no <resolution> clip at <aspect_ratio>`;
    - then `first_frame` and `last_frame`, each when it is not an `image/*` file: `<member> is <kind>, not a picture`.
  - The submit body is `{prompt, image_url, end_image_url?, aspect_ratio, resolution, duration}`, with `duration` a JSON integer.
  - The handle is `{"request_id", "status_path", "response_path"}`: the returned URLs minus `{queue}/`, kept byte for byte. A path is non-empty segments of `A-Z a-z 0-9 . _ ~ + = , @ -`, none of them `.` or `..`. It may end in `?` and a plain query: `name` or `name=value` pairs joined by `&`. A name is 1–64 of `A-Z a-z 0-9 . _ ~ -`; a value is up to 256 of those plus `+ , : @`. A name holding `auth`, `credential`, `expires`, `key`, `password`, `policy`, `secret`, `session`, `sig` or `token`, or starting `x-amz-` or `x-goog-`, is not kept. The whole reference is at most 1024 characters. `request_id` matches `^[A-Za-z0-9_.:-]{1,96}$`. A 2xx without such a `request_id` and two such URLs under the queue base is `Uncertain`. Its reason ends `(request <id>)` when the body's `request_id`, else the response's request-id header, is a safe id ([adapters.md](../../../../spec/adapters.md) §7).
  - `collect` polls `status_path` every 5 s for at most 1500 s. Each read is bounded by the smaller of the time left and 60 s. It then reads `response_path`, then downloads `video.url` once, at the URL exactly as fal gave it (https, no credential, at most 512 MiB). The bytes decide: the file is the clip when it carries the MP4 signature and an MP4 video track. A `content-type` header and the result's `content_type` are not read. The status and result reads are GETs with the credential and no `content-type`.
  - The answer is file `video` (`video/mp4`) with `data: {"facts": {"width", "height", "duration_seconds", "fps"}}`.
  - The check compares the clip's size and duration with the request. The size has a short side of 360, 720, 1080 or 2160, oriented by the aspect ratio. The duration must be within `1/fps + 0.01` s of the request.
- **Background removal.** Refused before sending: an `image` that is not an `image/*` file: `image is <kind>, not a picture`. The body is `{image_url, model: "General Use (Light)", operating_resolution: "1024x1024", output_mask: false, refine_foreground: true, output_format: png, mask_only: false, sync_mode: true}`. The answer is `root.image`: a data URL, or a `fal.media` URL as for images, downloaded with `accept: image/*` (a URL on any other host is never fetched, and the send fails as [adapters.md](../../../../spec/adapters.md) §3.4 says). It is file `image`, checked as a PNG. The deadline is 300 s.
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
