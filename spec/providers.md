# Providers

How FX talks to providers: the transport every adapter shares, where keys come from, how each provider answer becomes an attempt's outcome and its bill, long jobs, downloads, and the route table FX ships. What a capability's request and answer hold is [capabilities.md](capabilities.md). The engine's side (the cache, holds, job records, the retry owner) is [protocol.md](protocol.md) §6.1 and [store.md](store.md) §5.

Words: an **adapter** turns one capability call into provider requests. A **send** is one exchange of the call's request (or one submit of a long job). An **attempt** is one reserved, recorded and settled hold; a send that provably was not received is resent under the same attempt. Every rule below serves the ratified rule ([overview](../docs/wg/overview.md), "Retry and billing"): **the engine is the only retry owner, and a request is resent only when it provably was not received.**

## 1. Where adapters run

- The engine registers one adapter per capability and provider. A route `model@provider` is served by the adapter registered for its capability on its provider; the model is the route's, never chosen by the adapter.
- Adapters exist only in a **live** invocation (`grida-fx run --live`). Without `--live` nothing that can reach a provider is constructed: every call that is not answered from the call cache is refused (`not_live`), and a recorded call replays offline ([store.md](store.md) §4).
- A **stand-in run** ([protocol.md](protocol.md) §5.7) is never live, and uses only the adapters' checks: it holds each stand-in's answer to the check of the adapter registered for the route's capability and provider (§4.4). Those adapters are built with no keys over a transport that refuses every exchange (§2 item 8), and nothing but their checks is called.
- An adapter is constructed whether or not its provider's key is present. A call without the key is refused before anything is sent (§5).
- An adapter MUST NOT retry, pace, sleep (except while polling a long job, §7) or write to the store: it returns bytes and data, and the engine stores them. It MUST be safe to drop at any point: it starts no detached task and leaves nothing behind.

## 2. The transport

Adapters are thin clients over one injected transport. The transport MUST make **one HTTP exchange** per request and nothing else:

1. **No retries.** A failed exchange is reported, never repeated.
2. **No redirects.** A 3xx response is returned as a status. Following one could carry a credential to another host.
3. **One deadline per exchange**, set by the adapter, from connecting to the last byte of the response.
4. **A response cap**, set by the adapter. The transport counts the bytes it reads and fails the exchange as soon as the cap is crossed; a `content-length` above the cap fails before reading. Content is requested with `accept-encoding: identity`.
5. **Lanes.** Each request names its lane: `provider` (the provider's API), `upload` (files sent to the provider's API) or `download` (a result file at a URL the provider returned). A credential travels only on the provider and upload lanes; a download carries no credential, no cookie and no header the adapter did not set. On the download lane, a header named like a credential is refused as one: `authorization`, `proxy-authorization`, `cookie`, and any name containing `auth`, `cookie`, `key`, `token`, `secret`, `session`, `credential`, `password` or `signature`. On the other lanes, the credential's header is set only through the credential slot. A request that breaks this is refused before it leaves (`a download never carries a credential`).
6. **Credentials are attached by the transport** from the request's credential slot (`authorization: Bearer …`, `authorization: Key …`, `xi-api-key: …`). They are never part of the canonical request, a record, an event, a log line, a fixture or an error.
7. **Failures carry their phase.**
   - `not_sent`: the request provably never left. No connection was made (DNS, TCP connect, TLS handshake, a connect or pool timeout), or the transport refused it itself (a URL it cannot use, a credential on the download lane, the network turned off).
   - `after_send`: anything else. A reset, a read timeout, a truncated body and a response over the cap all are `after_send`.
   - A transport that cannot tell reports `after_send`.
8. **The network can be turned off.** With `GRIDA_FX_NETWORK=off`, a live invocation's transport refuses every exchange before it leaves (`not_sent`, refused). Tests of live paths use it; nothing is sent.
9. **Proxies never see a credential.** A request whose URL is plain `http`, or whose host is a loopback host (`localhost`, a loopback IP), goes straight to its host, whatever the environment's proxy settings say: a proxy would read a plain request, credential included, in clear text, and a loopback host is this machine, not the proxy's. Any other request is `https` to a remote host, and honours the proxy settings of the environment (`HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`; `HTTP_PROXY` names a proxy for plain `http` only, so it never applies). Through a proxy it is tunnelled (`CONNECT`): the proxy sees the host and port, and the request stays encrypted to the provider. These variables are the transport's; they are not keys or endpoints (§3).

The default transport is HTTP over rustls. Besides what the request sets, it sends `accept-encoding: identity`, `accept: */*` unless the request sets `accept`, and a `user-agent` on the provider and upload lanes. Tests use a replay transport that plays synthetic exchanges and asserts every request, or one that fails any send. The default transport MUST NOT be constructed in a test. A request, a response or a credential printed for debugging shows no URL path or query, no header value, no body and no key.

## 3. Keys and endpoints

| Provider | Key | Base URL | Default base |
|---|---|---|---|
| `openai` | `OPENAI_API_KEY` | `OPENAI_BASE_URL` | `https://api.openai.com/v1` |
| `openrouter` | `OPENROUTER_API_KEY` | `OPENROUTER_BASE_URL` | `https://openrouter.ai/api/v1` |
| `fal` | `FAL_KEY` | `FAL_BASE_URL` (the run host) | `https://fal.run`; queue `https://queue.fal.run` |
| `tripo` | `TRIPO_API_KEY` | none | `https://openapi.tripo3d.ai/v3` |
| `elevenlabs` | `ELEVENLABS_API_KEY` | `ELEVENLABS_BASE_URL` | `https://api.elevenlabs.io/v1` |

- **Only these nine variables are read.** A variable comes from the process environment when it is set and not blank (trimmed). Otherwise it comes from the planning project's `.env` file, if there is one. `GRIDA_FX_DISABLE_DOTENV=1` turns the file off.
- **The `.env` file is optional**, and it is read for these nine names only. Lines that name other variables are skipped without reading their values. It is refused when it is a symlink or not a regular file (`.env must be a regular file`), cannot be read (`.env could not be read`), or is not UTF-8 (`.env must be valid UTF-8`). Blank lines and `#` comments are skipped.
  - An assignment is `[export] NAME = value`. A line that starts with an allowed name and is not an assignment is refused: `.env contains malformed assignment for <NAME> on line <n>`.
  - An allowed name may appear only once: `.env contains duplicate key: <NAME>`.
  - A value may be unquoted (a trailing ` #` comment is removed, and a quote inside it is refused), `'single quoted'` (literal), or `"double quoted"` (a JSON string literal: `.env contains malformed quoted value for <NAME> on line <n>`). An empty value is refused: `.env contains an empty value for <NAME>`. A control character in the decoded value is refused: `.env contains an unsafe value for <NAME>`.
  - Every refusal names the variable and the line, never the value.
- **A base URL** is trimmed and loses a trailing `/`. It must be an `http(s)` URL with a host, no userinfo, no query, no fragment and a valid port, and `http` is allowed only for a loopback host. The refusals are `<Provider> base_url must be an HTTP(S) URL without credentials, query, or fragment`, `… must use a valid network port` and `… must use HTTPS unless it targets a loopback host`. A base URL that contains a key's value is refused: `<VARIABLE> holds a credential`. A base URL is configuration: it is not part of a route's fingerprint.
- **Key values are never printed.** `grida-fx doctor` says which keys are present and where each came from (environment or `.env`), never a value.

## 4. Outcomes

### 4.1 The outcomes of one send

| Outcome | Meaning | Billing | What the engine does next |
|---|---|---|---|
| `Refused` | refused before anything was sent (§5), or the transport refused the request itself | $0 | `capability_refused`, never retried |
| `NotReceived { retry_after }` | provably not received: the exchange failed `not_sent`, or the provider said it took nothing (§4.3) | $0 | resends under the same attempt (at most 6 sends in all), after its backoff or the provider's `retry_after` when that is longer, and never more than 60 s later (§4.3) |
| `Failed { cost, retryable }` | the provider received the request and the call failed | the reported cost, else the whole hold | a new attempt when `retryable` and sends are left; else `call_failed` |
| `Answered(answer)` | the provider answered | the answer's reported cost, else the whole hold | the adapter's check, then the engine's checks (§4.4) |

A long job's outcomes are in §7.

### 4.2 Transport failures

| Failure | Plain request | Long-job submit (the paid request) |
|---|---|---|
| `not_sent`, refused by the transport | `Refused` | `Refused` |
| `not_sent`, otherwise | `NotReceived` | `NotReceived` |
| `after_send` (a timeout, a reset, a truncated body, a body over the cap) | `Failed { cost: None, retryable: true }` | `Uncertain` |

### 4.3 HTTP statuses

The default classes are below. Each provider's table in §9 may override a row, and says where it does.

| Status | Plain request | Long-job submit (the paid request) |
|---|---|---|
| 2xx | parsed (§4.4) | parsed into a handle; a 2xx without a usable handle is `Uncertain` |
| 3xx | `Failed { cost: None, retryable: false }`: `<label> was redirected (HTTP <n>); check <BASE_URL variable>` | `Uncertain` |
| 408 | `NotReceived`: the server says it did not receive the whole request | `NotReceived` |
| 429 | `NotReceived`: the provider took nothing. Its `retry-after` header sets `retry_after` | `NotReceived` |
| any other 4xx | `Failed { cost, retryable: false }`: a deterministic refusal, so resending cannot help. `cost` is `Some(0)` only where §9 says the provider documents such refusals as unbilled; else `None` | `Failed { cost, retryable: false }`, the same way |
| 5xx | `Failed { cost: None, retryable: true }` | `Uncertain` |

`Some(0)` is never inferred. A provider's refusal is $0 only where its row in §9 says so, and §9 records why.

**The wait a provider asks for.**
- A `NotReceived` (or a submit's `NotReceived`) built from an HTTP response carries `retry_after` when the response asks for a wait: `retry-after-ms` in milliseconds, else `retry-after` in seconds (fractions allowed) or as an HTTP date. A header that cannot be read is ignored. A `NotReceived` from a transport failure carries none.
- The engine waits the longer of its backoff and `retry_after` before the next send, and never more than 60 s. The wait is pacing, not billing: it changes neither the outcome nor the attempt count.
- The reason may name the wait; it never quotes the response's other headers.

### 4.4 Answers and checks

- A 2xx whose body cannot become an answer (not JSON, a missing member, invalid base64, a URL where bytes were expected, a download that fails) is `Failed { cost, retryable: true }`. `cost` is the reported cost when the body was read far enough to find it, else `None`. The provider did the work.
- An answer is accepted only after three checks. The first is the **adapter's check**: judgements on a well-formed answer, such as the file's kind and signature, exact dimensions, alpha, or the answer against the request's schema. The engine then round-trips the answer's `data` through canonical JSON, and checks the capability's shape ([protocol.md](protocol.md) §6.1 step 7). A refusal from any of the three fails the attempt **as billed**, at the reported cost or else the whole hold, and the engine may make a new attempt.
- Structural failures belong in the send (`Failed`). Judgements on a well-formed answer belong in the check. Both bill and both may be retried; the split only decides where the sentence comes from.
- An answer MUST hold the files named in [capabilities.md](capabilities.md), with their kinds, and the `data` named there. It holds nothing else: no request id, no usage, no provider URL.
- A stand-in's answer passes the same checks, once and billed at nothing: its shape first, since no adapter made it, then the adapter's check, the round trip and the capability's ([protocol.md](protocol.md) §6.1, *Stand-in answers*). An adapter's check MUST therefore judge an answer from the call (its route, request, files and take) and the answer alone, never from anything its own send or collect kept.

## 5. Refusals before sending

An adapter MUST refuse a call before any byte leaves (`Refused`, $0, `capability_refused`) in this order. The first refusal wins.

1. **The route's contract is not one this adapter serves.** The `adapter` (and `adapter_behavior`, where §9 names one) in the route's contract MUST be the adapter's own when present: `<route> is not a <capability> route this adapter serves`. A member that is absent or `null` is not checked, so a route a project declares without a contract is served. The registry is keyed by capability and provider, not model, so this is the only guard against a table pointing a model at the wrong wire.
2. **The request does not fit its capability** ([capabilities.md](capabilities.md) §1). A member the capability does not define, a required member that is absent or `null`, or a member of the wrong type. The refusals of the later steps assume a request that passed this one: a required member is present, and every member has its type.
3. **The provider's key is missing**: `<VARIABLE> is not set`.
4. **A value the route cannot take**: blank text, a number out of range, an unknown enum value, a size the route does not serve, too many pictures, or a mask on a route without masks. Each provider's sentences are in §9.
5. **A file**: a file value whose digest the call does not carry, a store copy that cannot be read or is empty (`<member> has no bytes to send`), or a file of a kind the route does not take.

Refusals are $0 because nothing left. A provider that would have answered the same request with a 4xx is not asked.

## 6. Costs

- **A reported cost is the provider's own figure, in US dollars.** It must be a finite, non-negative JSON number. Booleans, strings and `null` are not costs. It is converted exactly from its JCS text ([identity.md](identity.md) §2) and MUST be **rounded up** to whole micro-dollars, so a cost is never under-counted: `0.00012345` is `$0.000124`, and `1e-7` is `$0.000001`. (This is the conversion of a reported figure. The half-even rounding of [identity.md](identity.md) §12 is for the engine's own arithmetic on prices.)
- **Credits.** A provider that reports credits converts them at its documented rate: Tripo, 1 credit = USD 0.01 (§9.4). The result is rounded up the same way.
- **Nothing is computed from tokens or price lists.** When the provider reports no cost, the cost is `None` and the engine charges the whole hold: never less than the call may have cost.
- **Collecting** a long job a previous run submitted bills nothing new ([protocol.md](protocol.md) §6.1 step 5). A job that ends without a result bills its whole hold in the run that submitted it, since `Ended` carries no cost.

## 7. Long jobs

A long job (a video, a mesh, a rig) is submitted once and then collected by its handle ([store.md](store.md) §5).

**Submit.**
- A submit may begin with a **free phase**: requests the provider does not bill, such as file uploads or a free pre-check. A failure there bills nothing:
  - `Refused` when it is deterministic (a 4xx other than 408 and 429, or a pre-check that refuses the input);
  - `NotReceived` otherwise. The engine resends the whole submit, free phase included.
- Then the **paid request** is sent exactly once. Its outcome follows §4.2 and §4.3:
  - `Accepted { handle }` when the provider took the job;
  - `NotReceived`, `Refused` or `Failed` when it provably did not, or refused it;
  - `Uncertain` when nobody can say whether it took the job. The job record then stays `submitting`, with the redacted reason as its `note` ([store.md](store.md) §5), the hold is charged in full, and the next run stops for a person (`job_unsettled`).
- **The handle holds only what collecting needs**: the provider's job id, paths relative to a fixed base, and facts the answer must report later. It MUST NOT hold a host name, a signed URL, an upload token or a credential.

**Collect.**
- `collect` MUST NOT submit. It reads the job's status every poll interval through the injected clock, until the job ends or the adapter's collect deadline passes, measured from the start of `collect`.
- Status reads are free and idempotent. They are polling, not sends: a read that fails (a transport failure, a 429, a 5xx, an unreadable body) is a poll that saw nothing, and polling goes on until the deadline.
- When polling ends:

  | Situation | Outcome | The job record |
  |---|---|---|
  | The job finished, its result files downloaded and passed their checks | `Answered` | removed after the call record is published |
  | The provider ended the job without a result (failed, cancelled, expired), or its result is unusable (no model, a file of the wrong kind) | `Ended` | `settled`; a later run submits the call anew |
  | Still running at the deadline, or every read failed until it | `Unreachable` | stays `submitted`; a later run collects again |
  | An answer about another job, or an unknown status | `Unreachable` | stays `submitted` |
  | A handle that is not one the adapter wrote | `Unreachable`, and nothing is requested | stays `submitted` |
  | A download that fails in a way a later collect may not repeat (a transport failure, a 408, a 429, a 5xx, an expired URL the provider will issue again) | `Unreachable` | stays `submitted` |
  | A credential or proxy problem (401, 403, a 3xx) | `Unreachable` | stays `submitted` |

  Settling a job that may still be running would invite a second paid submit, so whenever it is not clear that the job is over, the outcome is `Unreachable`.
- **The check of a collected answer** runs as for a plain answer. A refusal settles the record and fails the call, since drawing again is a new paid job.

## 8. Downloads and reasons

**Downloads.**
- A result file at a URL MUST be fetched **once** per send or collect, at the URL exactly as the provider gave it, on the download lane: no credential, no redirects, the response cap of §9, and the provider's host allowlist where §9 gives one. A failed fetch is not repeated within that send or collect. An adapter does not re-serialize the URL; it parses it only to check it. The default transport sends the URL as the WHATWG URL parser (Rust `url` crate, via reqwest, which has no raw-URI path) serializes it: in an http(s) query, controls, space, `"`, `<`, `>`, `'` and non-ASCII are percent-encoded; existing `%XX` escapes are kept; a default port is dropped, the host is lowercased, and `.`/`..` path segments are resolved.
- A download URL may be signed. It MUST NOT be kept in an answer's `data`, a handle, a record, an event or a reason. Adapters add it to the redactor before using it.
- A data URL in an answer is decoded in place; nothing is downloaded.

**Reasons.**
- Every sentence an adapter reports MUST be built from fixed text and allowlisted fields. Examples: `<label> returned HTTP <n>`, `: <safe detail>` (at most 720 characters), a request id that matches `^[A-Za-z0-9_.:-]{1,96}$`, and the bounded `type`/`code`/`param` of an OpenAI-style error envelope.
- It MUST NOT hold a key, an authorization header, a signed URL, an upload token, a data URL, a prompt or a response body.
- Every reason passes through redaction last. Each configured key and each added secret becomes `[redacted]`; known secret shapes are masked (data-URL payloads, base64 members such as `b64_json`, base64 runs of 80 or more characters, `sk-…` keys, `authorization` values, `api_key`/`token`/`secret`/`credential` members, URL queries). Whitespace is collapsed, and the result is cut to 500 characters ending in `…`. Text is redacted before it is cut, so a cut never leaves part of a secret.
- The engine's retry owner redacts once more every reason it reports (`capability_refused`, `call_failed`, `job_unsettled`), with a redactor over every key of the invocation: the adapter's own sentence, its check's, and the engine's checks of the answer, which may quote what the provider sent back (a `content-type`, a value the schema refused).
- `<label>` is the provider and the call, such as `OpenAI image generation` or `fal video submission`. Each adapter's labels are listed in §9.

## 9. Providers

Each provider's section is normative for its adapters. Status rows override §4.3 only where they say so.

### 9.1 OpenAI

- **Capabilities:** `image.generate` (`POST {base}/images/generations`, JSON) and `image.edit` (`POST {base}/images/edits`, multipart). One request per send. Contract `adapter`: `fx-openai-image-v1`, behaviour `"1"`, the string: the number `1` is another contract, with another fingerprint, and is refused (§5 step 1), as OpenRouter's image adapter refuses the number `3`.
- **Credential:** `authorization: Bearer <OPENAI_API_KEY>`.
- **Limits:** deadline 600 s per send; response cap 64 MiB.
- **Wire.** Always `n: 1`, `output_format: png`, `quality: max`, `moderation: low`, and `background` (including `auto`). `size` only when given (including `auto`). Never `input_fidelity` (the model refuses it), `response_format`, `seed` or `stream`.
  - An edit sends its text fields first, in that order.
  - Then each input image (`image` first, then `references`) as part `image[]`, named `reference-NN.<png|jpg|webp>` with `NN` from `01`.
  - Then the mask as part `mask`, named `reference-00.<ext>`.
- **Refusals (§5 step 4–5), in this order:**
  - a blank `prompt`: `an image call needs its prompt as text`;
  - a `background` other than `auto`, `opaque` or `transparent`: `background must be auto, opaque or transparent`;
  - a `size` that is not `auto` or `WxH`: `OpenAI image size must be auto or WIDTHxHEIGHT`; or one that leaves the envelope, checked in this order: edges multiples of 16 (`OpenAI image size edges must be multiples of 16`), the longer edge at most 3840 (`OpenAI image size edges must not exceed 3840 pixels`), aspect ratio at most 3:1 (`OpenAI image size aspect ratio must not exceed 3:1`), and 655 360 to 8 294 400 pixels (`OpenAI image size must contain between 655360 and 8294400 pixels`);
  - references on a generate (a non-empty `references`): `OpenAI image generation takes no references`;
  - more than 16 input images: `OpenAI image edits support at most 16 input images`;
  - an input or mask kind other than PNG, JPEG or WebP: `<member> is <kind>; OpenAI takes PNG, JPEG or WebP`; or bytes that do not decode as their kind: `<member> is not a decodable <kind>`. `<member>` is `image`, `references[<i>]` (counted from 0) or `mask`.
- **Answer.** `data[0].b64_json` is the one image. It must be strict base64, else the send is a structural failure; a `url` instead is a structural failure too, never fetched. Whether the bytes are a PNG is the check's judgement ([capabilities.md](capabilities.md) §2, check 1). The answer is file `image` (`image/png`) and `data: null`. Cost: `None`, since OpenAI reports token usage but no cost.
- **Check:** the image checks of [capabilities.md](capabilities.md) §2.
- **Statuses (overrides of §4.3):**
  - **408:** `Failed { cost: None, retryable: true }`. OpenAI's 408 is its own timeout, after the request arrived, so the image may have been drawn: the attempt is billed, and a new attempt may follow.
  - **4xx other than 408 and 429:** `Failed { cost: Some(0), retryable: false }`. OpenAI does not bill a request it rejects.
  - **429 with `error.code` or `error.type` `insufficient_quota`:** `Failed { cost: Some(0), retryable: false }`. The quota is exhausted, so the call ends.
  - **Any other 429:** `NotReceived` (§4.3); a `retry-after` that is a plain number of seconds is named in the reason (`; retry-after <n>`).
  - **400 whose `error.type` or `error.code` (ignoring case) is `moderation_blocked`, or contains `safety` or `content_policy`:** `Failed { cost: None, retryable: false }`. Moderation can run after generation, so the cost is unknown.
- **Reasons:** the label is `OpenAI image generation`, with the safe detail of the error envelope.

### 9.2 OpenRouter

- **Capabilities:**
  - `image.generate` and `image.edit` (`POST {base}/images`). Contract `adapter`: `fx-openrouter-image-v1`, behaviour `"3"`.
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

- **Route contract policy.** The structured and agent contracts take only `adapter`, `adapter_behavior`, `request_policy` and (structured only) `pictures`; another member is refused as `<route> is not a <capability> route this adapter serves: the contract takes no member <m>`, and so is a bad value, with its sentence after the colon. `request_policy` (optional) gives:
  - `provider`: `require_parameters` must be true, and is the default (`request_policy.provider.require_parameters must be true`); `only` is a non-empty list of distinct provider slugs (`request_policy.provider.only is a non-empty list of distinct provider slugs`); `allow_fallbacks` is a boolean (`request_policy.provider.allow_fallbacks is true or false`);
  - `reasoning.effort`: `request_policy.reasoning.effort is one of none, minimal, low, medium, high, xhigh, max`;
  - `image_detail`, which applies to every picture: `request_policy.image_detail is one of auto, low, high, original`.

  Objects must be objects (`request_policy is an object`, `request_policy.provider is an object`, `request_policy.reasoning is an object`) and take no other member (`<where> takes no member <m>`). `pictures: unchanged` sends structured pictures as they are (`pictures is "unchanged" or absent`). Otherwise each picture is fitted into 1600 × 1600, flattened on `matte`, and sent as PNG.
- **Images.** Refused before sending, in this order:
  - a blank `prompt`: `an image call needs its prompt as text`;
  - a mask: `OpenRouter image generation has no masked-edit route`;
  - `background: transparent` (the route serves no alpha): `OpenRouter image generation does not support transparent backgrounds`; a `background` other than `auto`, `opaque` or `transparent`: `background must be auto, opaque or transparent`;
  - a `size` other than `auto` and `1024x1024`, `1152x2496`, `1712x2560`, `2064x1008`, `2496x1152`, `2560x1440`, `2560x1712`: `OpenRouter serves no exact size <size>`;
  - references on a generate: `OpenRouter image generation takes no references`;
  - more than 16 pictures: `OpenRouter image edits support at most 16 input images`;
  - a picture that is not an `image/*` file: `<member> is <kind>, not a picture` (`image`, or `references[<i>]` counted from 0);
  - a body over 200 MiB: `request body exceeds 200 MiB`.

  The body is `{model, prompt, n: 1, provider: {allow_fallbacks: false, options: {openai: {moderation: low}}}, size?, quality: max, background, input_references?}`. The answer is `data[0].b64_json`, never a URL. A declared `media_type` must be a non-blank, parameter-free `image/png`, `image/jpeg` or `image/webp` (ignoring case; `image/jpg` and `null` are refused as structural failures); without that member the kind is sniffed from the bytes. The kind is kept as declared or sniffed, and the check refuses anything but PNG.
- **Structured output.**
  - The schema is a `json` file or an inline object. It is sent after local `$ref` inlining and strict canonicalization: `default` and the assertions strict mode refuses are dropped at schema positions, and `required` lists every property and `additionalProperties` is false. Schema positions are the root and, below a schema, each value of `properties`, `$defs`, `definitions` and `dependentSchemas`; `items`, `prefixItems`, `additionalItems`, `allOf`, `anyOf` and `oneOf`; `additionalProperties` when an object; `not`, `if`, `then`, `else` and `contains`. A property named like an assertion (`format`) is not one.
  - Refused before sending, in this order: a blank `prompt` (`a structured call needs its prompt as text`); a `matte` that is not `#rgb`, `#rrggbb` or `#rrggbbaa` (`matte <v> is not a colour`); a `max_tokens` below 1 (`max_tokens must be at least 1`) or above 2^53 − 1; a schema whose local references are unknown (`unknown local schema reference: <ref>`), cyclic (`cyclic local schema reference: <ref>`) or expand past a million values (`local schema references expand to more than 1000000 values`), or that does not compile (`a structured call's schema is not a JSON Schema (draft 2020-12)`); a schema file that is empty, not JSON or not an object (`schema has no bytes to send`, `a structured call's schema is not a JSON file`, `a structured call's schema is a JSON object`); a context file that is empty, not UTF-8 text or a picture that does not decode (`context <i> has no bytes to send`, `context <i> is not UTF-8 text`, `context <i> is not a decodable <kind>`); a body over 200 MiB (`request body exceeds 200 MiB`).
  - The response format's `json_schema.name` is the schema's `title` with every character but ASCII letters and digits written `_`, cut to 64 characters, else `answer`.
  - Text context files are appended to the prompt as `--- context <i> ---\n<text>`, where `<i>` counts every context file from 1, pictures included, and the text is sent as read.
  - The answer is `message.parsed`, else the JSON text of `message.content` (NaN and Infinity are refused; for a repeated member, the last value wins), unwrapped from `completionState` wrappers. Its `data` is `{"json": <value>}`.
  - The check validates the value against the request's original schema (draft 2020-12, `format` not asserted). It refuses with `<path>: <message>` for the error at the smallest instance path.
- **Agent turns.**
  - The engine's transcript maps to chat messages. Tool messages drop `name`, and their pictures are regrouped into a following user message `Pictures the tools returned.`. Each picture goes as a data URL of its own kind.
  - An assistant's `arguments` string is sent byte for byte.
  - Refused before sending: a call without an id (`a tool call without an id cannot be sent`); a tool message without `tool_call_id` (`a tool message without a tool_call_id cannot be sent`); a message whose role is not `user`, `assistant` or `tool` (`an agent transcript has no '<role>' messages`); a tool name that does not match `^[a-z][a-z0-9_]{0,63}$` (`tool name "<n>" must be lower_snake_case, at most 64 characters`); a blank description (`tool <name> must carry a description`); parameters that are not a schema object (`tool <name> parameters must be a JSON Schema object`); a picture that is not an `image/*` file (`messages[<i>].images[<j>] is <kind>, not a picture`, counted from 0; the kinds of every picture are judged before any file is read); a picture without bytes (`an agent picture has no bytes to send`); a `max_tokens` below 1; a body over 200 MiB. An empty id counts as none.
  - The answer is `data: {"text", "tool_calls": [{"id", "name", "arguments": <object>}]}`.
- **Music.** The prompt is the only input; `duration` stays in the call key but is not sent.
  - Refused before sending: a blank prompt (`a music call needs its prompt as text`) and a body over 200 MiB.
  - The response is read as SSE, or as one buffered JSON object. Audio chunks are concatenated, then strictly decoded once.
  - An empty body, an event that is not a JSON object, a stream with no events, an SSE error event, no audio, conflicting media types, or audio that is not MP3 is a structural failure.
  - The answer is file `audio`, checked against the MP3 signature.
- **Cost:** `usage.cost` (§6). For music, the last `usage` seen wins, an error event's included.
  - **A request on the user's own provider key (BYOK)**, which `usage.is_byok: true` or a non-null `usage.cost_details.upstream_inference_cost` shows: `usage.cost` is then OpenRouter's fee only, and the provider bills the inference to the user's own account. The cost is `cost` plus `upstream_inference_cost`, each rounded up (§6), so a ceiling counts what the call cost in all. A BYOK answer whose upstream cost is missing, `null` or not a cost reports `None`, and the engine charges the whole hold. Without BYOK, OpenRouter reports the upstream cost as `0` or `null`, and the cost is `cost`.
- **Statuses (overrides of §4.3):**
  - **408:** `Failed { cost: None, retryable: true }`, with or without an envelope. OpenRouter's 408 means the request timed out there, after it arrived, so the provider may have done the work: the attempt is billed, and a new attempt may follow.
  - **4xx other than 408 and 429, with an OpenRouter error envelope (`{"error": {...}}`):** `Failed { cost: Some(0), retryable: false }`. OpenRouter documents that an error status generates nothing and is not charged. Without an envelope, the cost is `None`.
  - **503 with an error envelope:** `NotReceived`. No provider meets the routing requirements yet. Without an envelope, a 503 follows §4.3.
- **Reasons:** the labels are `OpenRouter image generation`, `OpenRouter structured generation`, `OpenRouter tool loop` and `OpenRouter music generation`, each with the safe detail of the error envelope, which unwraps `error.metadata.raw`.

### 9.3 fal

- **Capabilities:**
  - `image.generate` (`POST {run}/<model>/text-to-image`) and `image.edit` (`POST {run}/<model>/edit`). Contract `adapter`: `fx-fal-image-v1`.
  - `video.generate`, a long job on the queue: `POST {queue}/<model>`. Contract `adapter`: `fal-queue`.
  - `background.remove` (`POST {run}/<model>`). Contract `adapter`: `fal-run-birefnet`.
- **Credential:** `authorization: Key <FAL_KEY>`, on the run and queue hosts only.
- **Images.**
  - Refused before sending, in this order:
    - a blank `prompt` (`an image call needs its prompt as text`), or one over 32 000 characters (`the prompt is longer than 32000 characters`);
    - a bad `background`: `background must be auto, opaque or transparent`;
    - references on a generate: `fal image generation takes no references`;
    - more than 16 pictures: `fal image edits support at most 16 input references`;
    - a `size` that is not `auto` or `WxH` (`fal image size must be auto or WIDTHxHEIGHT`), or outside the envelope of §9.1, with sentences starting `fal image size`;
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
  - The handle is `{"request_id", "status_path", "response_path"}`: the returned URLs minus `{queue}/`, kept byte for byte. A path is non-empty segments of `A-Z a-z 0-9 . _ ~ + = , @ -`, none of them `.` or `..`. It may end in `?` and a plain query: `name` or `name=value` pairs joined by `&`. A name is 1–64 of `A-Z a-z 0-9 . _ ~ -`; a value is up to 256 of those plus `+ , : @`. A name holding `auth`, `credential`, `expires`, `key`, `password`, `policy`, `secret`, `session`, `sig` or `token`, or starting `x-amz-` or `x-goog-`, is not kept. The whole reference is at most 1024 characters. `request_id` matches `^[A-Za-z0-9_.:-]{1,96}$`. A 2xx without such a `request_id` and two such URLs under the queue base is `Uncertain`. Its reason ends `(request <id>)` when the body's `request_id`, else the response's request-id header, is a safe id (§8).
  - `collect` polls `status_path` every 5 s for at most 1500 s. Each read is bounded by the smaller of the time left and 60 s. It then reads `response_path`, then downloads `video.url` once, at the URL exactly as fal gave it (https, no credential, at most 512 MiB). The bytes decide: the file is the clip when it carries the MP4 signature and an MP4 video track. A `content-type` header and the result's `content_type` are not read. The status and result reads are GETs with the credential and no `content-type`.
  - The answer is file `video` (`video/mp4`) with `data: {"facts": {"width", "height", "duration_seconds", "fps"}}`.
  - The check compares the clip's size and duration with the request. The size has a short side of 360, 720, 1080 or 2160, oriented by the aspect ratio. The duration must be within `1/fps + 0.01` s of the request.
- **Background removal.** Refused before sending: an `image` that is not an `image/*` file: `image is <kind>, not a picture`. The body is `{image_url, model: "General Use (Light)", operating_resolution: "1024x1024", output_mask: false, refine_foreground: true, output_format: png, mask_only: false, sync_mode: true}`. The answer is `root.image`: a data URL, or a `fal.media` URL as for images, downloaded with `accept: image/*` (a URL on any other host is never fetched, and the send fails as §4.4 says). It is file `image`, checked as a PNG. The deadline is 300 s.
- **Cost:** `usage.cost` at the payload's top level (§6), else `None`.
- **Statuses on the run host (overrides of §4.3):**
  - **4xx other than 408 and 429:** `Failed { cost: Some(0), retryable: false }`. fal validates a request before running it, and does not bill a refusal.
- **Statuses of the video submit:**
  - **4xx other than 408 and 429:** `Failed { cost: Some(0), retryable: false }`.
  - **500, 502 and 503:** `NotReceived`. fal answers these without taking the job.
  - **504 and other 5xx:** `Uncertain`.
- **Collect statuses:**
  - A status read of 404, 410 or another 4xx (not 401, 403, 408 or 429), or a status with a truthy `error`, is `Ended`. Its reason gives `error_type` and the error text, each redacted before it is cut (to 100 and 500 characters).
  - A 408, a 429, a 5xx, a transport failure or a body that is not a JSON object is a failed status or result read, and polling goes on.
  - A 401, 403 or 3xx on a status or result read is `Unreachable`, and so is a status that names another `request_id`.
  - The download is made once per collect (§8). A transport failure, a refusal by the transport, a 3xx, 401, 403, 408, 429, a 5xx or another status outside 2xx and 4xx is `Unreachable`; a later collect reads the result and downloads again. Another 4xx is `Ended`, and so is a body that is empty, over the cap or not MP4 by its bytes, or a clip with no video stream.
  - A handle that is not one the adapter wrote (`a fal video job handle is not one this adapter wrote`) is `Unreachable`, and nothing is requested.
  - A fal job that may still be running is never settled.
- **Reasons:** the labels are `fal image generation`, `fal output image download`, `fal video submission`, `fal video job status`, `fal video job result`, `fal output video download` and `fal background removal`. Response bodies are never quoted, except the bounded `error_type` and error of a failed job.

### 9.4 Tripo

- **Capabilities:**
  - `mesh.generate`, a long job: the free phase uploads the views, then `POST /generation/multiview-to-model`. Contract `adapter`: `tripo-multiview`.
  - `mesh.rig`, a long job: the free phase uploads the model, posts `POST /animations/rig-check` and waits for it; then `POST /animations/rig`. Contract `adapter`: `tripo-rig`.
- **Credential:** `authorization: Bearer <TRIPO_API_KEY>` on every API request, and never on a model download.
- **The envelope.** Every API answer must be HTTP 200 exactly, with a JSON object `{"code": 0, "data": {...}}`. Tripo's error text is never read: a reason says `body withheld`. A refusal (below) names Tripo's error code when the body is a JSON object whose `code` is an integer, as ` (code <n>)` after the status (`Tripo refused the task with HTTP 400 (code 2002)`); nothing else of the body is kept.
- **Limits:**
  - each upload, status read and download: 300 s;
  - the paid POST: 180 s;
  - the mesh collect: 1200 s; the rig check and the rig collect: 600 s each;
  - the poll interval: 5 s;
  - each model download: at most 150 000 000 bytes.
- **Mesh.**
  - Refused before sending, in this order: no views (`a mesh call needs its views, by name`); no `front`, or a view other than front, back, left or right (`a multiview task takes front and any of back, left, right`, followed by `; not <names>` listing the unknown names, sorted); a `face_limit` outside 48 to 25 000 (`face_limit is 48 to 25000`); then, in the order front, back, left, right, a view that is not PNG or JPEG (`the <view> view is <kind>; Tripo takes PNG or JPEG`) or has no bytes (§5 step 5).
  - The views are uploaded in the order front, back, left, right (part `file`, named `<view>.png` or `<view>.jpg`).
  - The paid body is `{model, quad, texture, pbr, face_limit?, inputs: [{"<view>": <token>}, …]}`, with `quad` false, `texture` true and `pbr` false when absent.
  - The handle is `{"task_id"}`.
- **Rig.**
  - Refused before sending: a model the call does not carry or that has no bytes (`a rig call needs its model`), or one that is not `model/gltf-binary` (`Tripo rigs a GLB, not <kind>`).
  - The model is uploaded as `unrigged.glb`.
  - The check task must succeed with a boolean `riggable`. A doubted model, where `riggable` is not true or the check's `rig_type` differs from the request's (default `biped`), is refused unless `allow_negative_check` is true.
  - The paid body is `{input, model, rig_type, spec: <skeleton, default mixamo>, out_format: glb}`.
  - The handle is `{"task_id", "check": {"task_id", "riggable", "rig_type"}, "advisory_override"}`. The handle's `check.rig_type` is the check's `rig_type` when it is a string matching `^[A-Za-z0-9_.:-]{1,96}$`, else `null`. The answer's `checked_rig_type` is read from the handle the same way.
- **Free phase:**
  - 400, 401, 403, 404, 413, 415 and 422 are `Refused`, and so is a check task that ends without success;
  - every other failure is `NotReceived`: a transport failure, 429, 5xx, an unreadable envelope, a missing token or task id, a check still running at its deadline, or a non-boolean `riggable`.
- **Paid POST (overrides of §4.2–4.3):**
  - a request the transport refuses itself: `Refused`;
  - `not_sent` otherwise, and 429: `NotReceived`;
  - 400, 401, 403, 404 and 422: `Failed { cost: None, retryable: false }`. Tripo has not documented that a refused task is unbilled;
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
  - an unknown status, another task's answer, and a download that fails or is over the cap are `Unreachable`;
  - a 401, a 403, a 3xx or a transport refusal on a status read ends polling at once as `Unreachable`; other failed reads (404 and 429 included) are polls that saw nothing.
- **Cost:** `credits_consumed` of the finished task, a number or a decimal string, × USD 0.01, rounded up (§6). The check's credits are not counted.

### 9.5 ElevenLabs

- **Capabilities:**
  - `sound.generate`: `POST {base}/sound-generation?output_format=mp3_44100_192`. Contract `adapter`: `elevenlabs-sound-effect`, behaviour `1`.
  - `speech.generate`: `POST {base}/text-to-speech/<voice>?output_format=mp3_44100_192`. Contract `adapter`: `elevenlabs-speech`, behaviour `1`.
- **Credential:** `xi-api-key: <ELEVENLABS_API_KEY>`, plus headers `content-type: application/json` and `accept: audio/mpeg`.
- **Limits:** deadline 120 s; response cap 64 MiB.
- **Sound.**
  - Refused before sending, in this order: a blank prompt (`a sound call needs its prompt as text`), or one over 450 characters (`sound effect prompt must be at most 450 characters`); a `duration` outside 0.5 to 30 (`duration must be between 0.5 and 30`); a `prompt_influence` outside 0 to 1 (`prompt_influence must be between 0 and 1`).
  - The body is `{text, model_id: <route model>, loop, duration_seconds?, prompt_influence?}`, with `loop` false when absent.
- **Speech.**
  - Refused before sending, in this order: blank text (`a speech call needs its text`); a blank voice (`a speech call needs its provider voice`); a voice that does not match `^[A-Za-z0-9_-]{1,128}$` (`the provider voice is not a voice id`); text over 5000 characters (`speech text must be at most 5000 characters`); a `stability` outside 0 to 1 (`stability must be between 0 and 1`); a `language_code` that is blank but not `""` (`language_code must not be blank`).
  - The body is `{text, model_id, voice_settings?: {stability}, language_code?}`. `language_code` `null` or `""` is not sent. `max_chars` is for pricing only and is never sent.
- **Answer:** the response body is the audio, as file `audio`. Its kind is the normalized `content-type`, or `audio/mpeg` when that header is absent. A `content-type` outside the audio family is kept as its lowercased base type, or as `application/octet-stream` when it is not a media type, so the check refuses it. `data: null`. Cost: `None`, since ElevenLabs reports characters, not dollars.
- **Check, in order:**
  - not empty: `<label> returned no audio data`;
  - an MP3: `requested mp3 but received <kind>`;
  - the MP3 signature: `audio bytes do not match declared media type audio/mpeg`.
- **Statuses:** §4.3 as written: a 408 is `NotReceived` (ElevenLabs did not read the whole request), and a 4xx other than 408 and 429 costs `None`.
- **Reasons:** the labels are `ElevenLabs sound generation` and `ElevenLabs speech generation`. The error body's `detail.message` is never quoted.

## 10. The built-in route table

FX ships a default route table, embedded in `grida-fx`. It is the first table of the catalog ([identity.md](identity.md) §7): a project's `route_tables` override its entries, and any `--routes` file leaves it out.

- It holds 16 routes over 10 capabilities. Every route MUST be served by an adapter of §9, and every capability is defined in [capabilities.md](capabilities.md).
- **Prices are planning allowances** in US dollars, not provider quotes. What a call costs is settled from what the provider reports, or else from the whole hold (§6). The video route is priced per second, in tiers by `resolution`.
- **Contracts are identity.** Each contract object enters its route fingerprint, and so every cache key the route serves. Changing a contract changes every key under it.
  - A contract's `adapter` is the name §9 gives the adapter that serves the route, and its `adapter_behavior` is the behaviour §9 names, where §9 names one.
  - The image routes carry `adapter_behavior` as a string (`"1"`, `"3"`), and every other route carries it as the integer `1`. Each adapter serves its behaviour only as spelled here: the string and the number are different contracts, with different fingerprints.
  - The two `openai/gpt-6-astra` routes carry the `request_policy` of §9.2, and the structured one also `pictures: unchanged`.
- **Features** are the names [capabilities.md](capabilities.md) lists for each capability, and a route declares only what its adapter honours. So the OpenRouter image routes declare neither `alpha` nor `mask`, and no `image.generate` route declares `image_input`, since every one refuses references (§9).
- **Pacing is not identity.** The OpenAI and OpenRouter image routes declare `requests_per_minute: 150`, which the engine applies per route. The Tripo routes and the video route declare `concurrency: 1`. The structured routes declare `concurrency: 4`, and the `openai/gpt-6-astra` agent route declares `concurrency: 1`.
- **Shipped without a route:** `background.remove` has an adapter on fal, but no route until fal's price for it is recorded. `vision.annotate`, `vision.review` and `structured.review` have no adapter. Calls on any of these are `no_route` unless a project's table adds a route.

## 11. Changes from stage-gen's engine

- **Every attempt is held and settled.** Before, six attempts shared one hold, so billed attempts went uncounted.
- **Refusals before sending cost $0.** Before, some were charged the whole hold: a missing image key, a bad `background`, references on a generate, an unknown `$ref`, a bad schema file, an out-of-range sound duration, and free-phase failures of Tripo.
- **Deterministic 4xx answers end the call.** Before, they were resent up to six times.
- **Pacing is the engine's, per route.** Before, an image adapter paced only within one call.
- **A rate-limited send waits as long as the provider asks**, up to 60 s (§4.3). Before, `retry-after` was never read.
- **Tripo jobs are no longer forgotten.** Before, a Tripo job still running at the deadline, or with a malformed status, was settled, and the next run paid again. Now it is `Unreachable` and stays `submitted`.
- **A provably unsent Tripo task post is `NotReceived`.** Before, it stopped the next run for a person.
- **The fal handle holds paths**, with a plain query kept, and a `request_id` that is a safe id. Before, it held absolute URLs and any `request_id`. A failed status read during a fal collect no longer settles the job.
- **Downloads are capped.** Video, background and Tripo downloads have size caps, and every download refuses redirects and carries no credential.
- **Music cost is read.** OpenRouter music now reads `usage.cost`; before, every call was charged its whole hold.
- **A malformed `revised_prompt` no longer fails a paid image.**
- **Answers no longer carry `attempts`**, request ids or usage. Structured output answers `{"json": value}`.
- **A request timed out at OpenAI or OpenRouter (408) is billed.** It arrived, and the provider may have done the work: `Failed { cost: None, retryable: true }` (§9.1, §9.2), never a free resend.
- **OpenRouter BYOK costs count the upstream charge** (§9.2). Before, only `usage.cost` was read, OpenRouter's fee alone, so a ceiling saw $0 for paid upstream work.
- **Proxies never carry a credential in clear text** (§2 item 9). Before, the environment's `HTTP_PROXY` also took plain `http` requests to a loopback base URL, key included.
- **A download refuses any header named like a credential** (§2 item 5), not only the credential slot.
- **Agent turns refuse a file that is not a picture** (§9.2). Before, it was sent as `image/png`.
- **Structured output's schema `name`** is the schema's `title` with every character but ASCII letters and digits written `_`, at most 64 characters, else `answer`. Before, it was the title, else the schema file's stem, and non-ASCII letters were kept (`Café plan` was `Café_plan`, and is now `Caf__plan`).
- **Structured output's text context** is appended under `--- context <i> ---`, where `<i>` counts every context file from 1, pictures included, and the text goes as read, `\r\n` included. Before, the header was `--- <file name> ---`, and CRLF became LF.
- **Strict schemas are canonicalized at schema positions only** (§9.2). A property named `format`, `default`, `pattern` or `properties`, and the contents of `enum` and `const`, are kept. Before, they were deleted wherever they occurred, and `required` could list properties the schema no longer had.
- **Reduced structured pictures are scaled from their full depth.** A 16-bit picture is scaled as such; before, it was clipped to white. The Lanczos filters differ by under 1/255 in mean.
- **`null` counts as absent** ([capabilities.md](capabilities.md) §1). Tripo's `texture: null` sends `true`, the default, where the predecessor sent `false`; fal video's `aspect_ratio: null` draws at `9:16`, where the predecessor refused.
- **fal requests carry only the headers they need.** The queue's status and result reads carry no `content-type`, and a background-removal result is downloaded with `accept: image/*`, not `*/*`.
- **fal result hosts are held to §9.3.** A background-removal result is downloaded only from `fal.media` (before, from any host), and a video only over `https` (before, `http` too).
- **`ELEVENLABS_BASE_URL` is honoured** (§3). Before, the ElevenLabs routes always went to the default base.
- **fal pictures must be pictures.** A file that is not `image/*` is refused before sending, at $0, for images, video frames and background removal. Before, it was sent labelled `image/png`.
- **fal clips are judged by their bytes**, not by `content-type`. Before, a valid MP4 served as octet-stream, or with no type, was ended and paid for again.
- **A fal clip download is made once per collect;** a failure leaves the job `submitted`.
- **A Tripo check `rig_type` is kept only as a short plain string**, else `null`.
- **An Uncertain submit names the provider's job id** when it is safe, and its job record keeps the reason as its `note` ([store.md](store.md) §5), so `grida-fx jobs` and a later run's `job_unsettled` show it. Before, the job record kept nothing and a later run could not say which job to look for.
- **Provider URLs reach the transport as given;** the default transport sends `'` in a query as `%27` (httpx sent it as is).
- **Route contracts and features use FX's names.** The image routes' contracts name FX's adapters (`fx-openai-image-v1`, `fx-openrouter-image-v1`, `fx-fal-image-v1`), so their fingerprints differ from the predecessor's. The predecessor's `route_id` and `surface` members, which no adapter reads, are dropped. A feature has one name: the aliases `masked_edit`, `transparent_background`, `reference_images` and `data_url_reference_input` are gone, and the OpenRouter `image.generate` route no longer claims to take input pictures. A step that still requires an alias is refused while planning as no feature of its capability, naming FX's: `alpha`, `mask`, `image_input`.
