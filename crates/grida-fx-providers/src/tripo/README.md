# Tripo

The reference for FX's adapters on `tripo`, under the adapter contract, [spec/adapters.md](../../../../spec/adapters.md), whose sections it cites; it is normative for these adapters. [spec/providers.md](../../../../spec/providers.md) §1, §2 and §3 hold their key and endpoint, the built-in adapters and the routes FX ships.

| Capability | Contract `adapter` | `adapter_behavior` |
|---|---|---|
| `mesh.generate` | `tripo-multiview` | `1` |
| `mesh.rig` | `tripo-rig` | `1` |

A route whose contract names another `adapter` or `adapter_behavior` than the ones above is refused ([adapters.md](../../../../spec/adapters.md) §4 step 1). Up to FX 0.6.1, these adapters checked only `adapter`.

- **Capabilities:**
  - `mesh.generate`, a long job: the free phase uploads the views, then `POST /generation/multiview-to-model`. Contract `adapter`: `tripo-multiview`, behaviour `1`.
  - `mesh.rig`, a long job: the free phase uploads the model, posts `POST /animations/rig-check` and waits for it; then `POST /animations/rig`. Contract `adapter`: `tripo-rig`, behaviour `1`.
- **Credential:** `authorization: Bearer <TRIPO_API_KEY>` on every API request, and never on a model download.
- **Provider options** ([adapters.md](../../../../spec/adapters.md) §8): the parameters of `provider_options.tripo` are added at the end of the paid body, never to an upload or the rig check. They are refused before the free phase, as the first of the values below, when they set a member of that body: `model`, `quad`, `texture`, `pbr`, `face_limit` and `inputs` for a mesh; `input`, `model`, `rig_type`, `spec` and `out_format` for a rig.
- **The envelope.** Every API answer must be HTTP 200 exactly, with a JSON object `{"code": 0, "data": {...}}`. Tripo's error text is never read: a reason says `body withheld`. A refusal (below) names Tripo's error code when the body is a JSON object whose `code` is an integer, as ` (code <n>)` after the status (`Tripo refused the task with HTTP 400 (code 2002)`); nothing else of the body is kept.
- **Limits:**
  - each upload, status read and download: 300 s;
  - the paid POST: 180 s;
  - the mesh collect: 1200 s; the rig check and the rig collect: 600 s each;
  - the poll interval: 5 s;
  - each model download: at most 150 000 000 bytes.
- **Mesh.**
  - Refused before sending, in this order: provider options (above); no views (`a mesh call needs its views, by name`); no `front`, or a view other than front, back, left or right (`a multiview task takes front and any of back, left, right`, followed by `; not <names>` listing the unknown names, sorted); `front` alone (`a multiview task takes front and at least one of back, left, right`); a `face_limit` outside 48 to 25 000 (`face_limit is 48 to 25000`); then, in the order front, back, left, right, a view that is not PNG or JPEG (`the <view> view is <kind>; Tripo takes PNG or JPEG`) or has no bytes ([adapters.md](../../../../spec/adapters.md) §4 step 5).
  - The views are uploaded in the order front, back, left, right (part `file`, named `<view>.png` or `<view>.jpg`).
  - The paid body is `{model, quad, texture, pbr, face_limit?, inputs: [{"<view>": <token>}, …]}`, with `quad` false, `texture` true and `pbr` false when absent, then the provider options.
  - The handle is `{"task_id"}`.
- **Rig.**
  - Refused before sending: provider options (above); a model the call does not carry or that has no bytes (`a rig call needs its model`), or one that is not `model/gltf-binary` (`Tripo rigs a GLB, not <kind>`).
  - The model is uploaded as `unrigged.glb`.
  - The check task must succeed with a boolean `riggable`. A doubted model, where `riggable` is not true or the check's `rig_type` differs from the request's (default `biped`), is refused unless `allow_negative_check` is true.
  - The paid body is `{input, model, rig_type, spec: <skeleton, default mixamo>, out_format: glb}`, then the provider options.
  - The handle is `{"task_id", "check": {"task_id", "riggable", "rig_type"}, "advisory_override"}`. The handle's `check.rig_type` is the check's `rig_type` when it is a string matching `^[A-Za-z0-9_.:-]{1,96}$`, else `null`. The answer's `checked_rig_type` is read from the handle the same way.
- **Free phase:**
  - 400, 401, 403, 404, 413, 415 and 422 are `Refused`, and so is a check task that ends without success;
  - every other failure is `NotReceived`: a transport failure, 429, 5xx, an unreadable envelope, a missing token or task id, a check still running at its deadline, or a non-boolean `riggable`.
- **Paid POST (overrides of [adapters.md](../../../../spec/adapters.md) §3.2–3.3):**
  - a request the transport refuses itself: `Refused`;
  - `not_sent` otherwise, and 429: `NotReceived`;
  - 400, 401, 403, 404 and 422: `Failed { cost: None, retryable: false }`. Tripo's [billing documentation](https://developers.tripo3d.com/en/docs/billing) says credits are frozen when a task is created, and its [task lifecycle](https://developers.tripo3d.com/en/docs/task-lifecycle) says a submitted request returns a `task_id`. A refused POST returns none. But Tripo does not document a refusal as unbilled, and `Some(0)` is never inferred ([adapters.md](../../../../spec/adapters.md) §3.3);
  - `after_send`, 3xx, 408, 5xx, any other status, an unreadable envelope, and a missing or malformed task id: `Uncertain`. The reason adds the cause in brackets (`Tripo may have taken the task; it is not posted again (<cause>)`).
  - A task id matches `^[A-Za-z0-9_-]{1,128}$`. An `Uncertain` for a 200 whose task id is a string matching `^[A-Za-z0-9_.:-]{1,96}$` names it: `Tripo's answer names task <id>, which is not a task id FX collects`. Any other non-blank string gives `Tripo's answer has a malformed task id`; none gives `Tripo's answer has no task id`.
- **Tasks.**
  - `GET /tasks/<id>` must answer for the same task id, with a status of `queued`, `running`, `success`, `failed`, `cancelled`, `banned` or `expired`.
  - Model URLs are the `https://` strings under object keys (arrays are not entered; keys match `[A-Za-z0-9_]+`) whose path names `model` or `mesh`. There are 1 to 8 distinct URLs, each at most 20 480 characters with no control character.
  - Downloads go only to `tripo3d.ai`, its subdomains, and `tripo-data.rg1.data.tripo3d.com` (https, port 443, no userinfo). Each model is downloaded at Tripo's URL, byte for byte. Each file is sniffed: `glTF` is GLB, `Kaydara FBX Binary` is FBX, and anything else ends the job.
  - A mesh answer keeps the first FBX, else the first GLB, as file `model`, with `data: {"facts": {"model_kind"}}`.
  - A rig answer needs exactly one GLB, as file `model`, with `data: {"facts": {"riggable", "checked_rig_type", "advisory_override"}}` taken from the handle.
- **Collect:**
  - `failed`, `cancelled`, `banned` and `expired` are `Ended`, and so are an unusable URL scan, a URL off Tripo's hosts, and a file of the wrong kind;
  - an `Ended` for `failed` or `cancelled` costs the `credits_consumed` of that status read, converted as under **Cost**, and `Some(0)` when the read has none (absent or `null`). A figure that cannot be read costs `None`. Tripo's [billing documentation](https://developers.tripo3d.com/en/docs/billing) says `Failed and cancelled tasks are not charged`, and its [task lifecycle](https://developers.tripo3d.com/en/docs/task-lifecycle) says the frozen credits of a failed or cancelled task are released. A figure Tripo reports still wins over that rule ([adapters.md](../../../../spec/adapters.md) §5). An `Ended` for an unusable output (the URL scan, a host, a file's kind, or no model an answer can keep) comes from a task that succeeded, so it costs that success read's `credits_consumed`, as an answer would (**Cost**). `banned` and `expired` cost `None`, whatever the read reports: Tripo documents nothing for them;
  - an unknown status, another task's answer, and a download that fails or is over the cap are `Unreachable`;
  - a 401, a 403, a 3xx or a transport refusal on a status read ends polling at once as `Unreachable`; other failed reads (404 and 429 included) are polls that saw nothing.
- **Cost:** `credits_consumed` of the finished task, a number or a decimal string, × USD 0.01, rounded up ([adapters.md](../../../../spec/adapters.md) §5), whether its output answers or is unusable; `None` when it is absent or cannot be read. The check's credits are not counted. A task that ended `failed` or `cancelled` costs its own read's `credits_consumed` the same way, or $0 when that read reports none (above). A refused paid POST, and a `banned` or `expired` task, are charged the whole hold. These costs are booked by the run that submitted the task, when it sees the task end. A task still running when that run stops waiting keeps the whole hold that run charged, and a later run that finds it ended bills nothing new ([adapters.md](../../../../spec/adapters.md) §5).
