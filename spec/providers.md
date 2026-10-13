# Providers

How FX talks to providers: the transport every adapter shares, where keys come from, how each provider answer becomes an attempt's outcome and its bill, long jobs, downloads, and the route table FX ships. What a capability's request and answer hold is [capabilities.md](capabilities.md). The engine's side (the cache, holds, job records, the retry owner) is [protocol.md](protocol.md) §6.1 and [store.md](store.md) §5.

Words: an **adapter** turns one capability call into provider requests. A **send** is one exchange of the call's request (or one submit of a long job). An **attempt** is one reserved, recorded and settled hold; a send that provably was not received is resent under the same attempt. Every rule below serves the ratified rule ([overview](../docs/wg/overview.md), "Retry and billing"): **the engine is the only retry owner, and a request is resent only when it provably was not received.**

## 1. Where adapters run

- The engine registers one adapter per capability and provider. A route `model@provider` is served by the adapter registered for its capability on its provider; the model is the route's, never chosen by the adapter.
- Adapters exist only in a **live** invocation (`grida-fx run --live`). Without `--live` nothing that can reach a provider is constructed: every call that is not answered from the call cache is refused (`not_live`), and a recorded call replays offline ([store.md](store.md) §4).
- A **stand-in run** ([protocol.md](protocol.md) §5.7) is never live, and uses only the adapters' checks: it holds each stand-in's answer to the check of the adapter registered for the route's capability and provider (§4.4). Those adapters are built with no keys over a transport that refuses every exchange (§2 item 8), and nothing but their checks is called.
- An adapter is constructed whether or not its provider's key is present. A call without the key is refused before anything is sent (§5).
- **Runtimes.** Some adapters work through a *runtime*: a program the user installed and signed into, which the invocation starts on the first call that needs it, shares between that call and the rest, and shuts down with the run. The engine owns a runtime's life: the run's cleanup shuts it down and counts it in its verification ([control.md](control.md)), and a command that exits on an interrupt kills it. A runtime may keep tasks of its own (reading and writing the program's pipes); they end with it. Codex (§9.6) and Grok Build (§9.8) are two, and none of §2 applies to them: only a live invocation looks for their programs to start them, and only when the network is on (§2 item 8), so no other invocation and no test can start it. A call through a runtime is safe to drop as every adapter's is: what it asked the runtime for is stopped.
- An adapter MUST NOT retry, pace, sleep (except while polling a long job, §7, or waiting for the charge of a request it made, §9.7) or write to the store: it returns bytes and data, and the engine stores them. It MUST be safe to drop at any point: it starts no detached task and leaves nothing behind.

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
| `gg` | `GG_API_KEY` | `GG_BASE_URL` | `https://gateway.grida.ai/v1` |
| `openai` | `OPENAI_API_KEY` | `OPENAI_BASE_URL` | `https://api.openai.com/v1` |
| `openrouter` | `OPENROUTER_API_KEY` | `OPENROUTER_BASE_URL` | `https://openrouter.ai/api/v1` |
| `fal` | `FAL_KEY` | `FAL_BASE_URL` (the run host) | `https://fal.run`; queue `https://queue.fal.run` |
| `tripo` | `TRIPO_API_KEY` | none | `https://openapi.tripo3d.ai/v3` |
| `elevenlabs` | `ELEVENLABS_API_KEY` | `ELEVENLABS_BASE_URL` | `https://api.elevenlabs.io/v1` |

- **Only these eleven variables are read.** A variable comes from the process environment when it is set and not blank (trimmed). Otherwise it comes from the planning project's `.env` file, if there is one. `GRIDA_FX_DISABLE_DOTENV=1` turns the file off.
- **The `.env` file is optional**, and it is read for these eleven names only. Lines that name other variables are skipped without reading their values. It is refused when it is a symlink or not a regular file (`.env must be a regular file`), cannot be read (`.env could not be read`), or is not UTF-8 (`.env must be valid UTF-8`). Blank lines and `#` comments are skipped.
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
3. **The provider's key is missing**: `<VARIABLE> is not set`. For Codex and Grok Build, which have no key, their program is missing instead (§9.6, §9.8).
4. **A value the route cannot take**: blank text, a number out of range, an unknown enum value, a size the route does not serve, too many pictures, or a mask on a route without masks. Each provider's sentences are in §9.
5. **A file**: a file value whose digest the call does not carry, a store copy that cannot be read or is empty (`<member> has no bytes to send`), or a file of a kind the route does not take.

Refusals are $0 because nothing left. A provider that would have answered the same request with a 4xx is not asked.

## 6. Costs

- **Billing.** An adapter says how its calls are billed: in **dollars**, to the account of the key FX sends (every provider but one), or against the user's own **plan** in a runtime they signed into (Codex, §9.6, and Grok Build, §9.8), which FX neither sees nor books. Only an adapter makes a provider bill a plan, never a route table. A live run whose calls are all on providers that bill a plan needs no ceiling: it runs under a ceiling of $0, and refuses a call on any other provider ([protocol.md](protocol.md) §6.1). An answer may also report what the call consumed besides dollars (tokens, the runtime and its version); the call record keeps it as `usage` ([store.md](store.md) §3).
- **A reported cost is the provider's own figure, in US dollars.** It must be a finite, non-negative JSON number. Booleans, strings and `null` are not costs. It is converted exactly from its JCS text ([identity.md](identity.md) §2) and MUST be **rounded up** to whole micro-dollars, so a cost is never under-counted: `0.00012345` is `$0.000124`, and `1e-7` is `$0.000001`. (This is the conversion of a reported figure. The half-even rounding of [identity.md](identity.md) §12 is for the engine's own arithmetic on prices.)
- **Credits.** A provider that reports credits converts them at its documented rate: Tripo, 1 credit = USD 0.01 (§9.4). The result is rounded up the same way.
- **Nothing is computed from tokens or price lists.** When the provider reports no cost, the cost is `None` and the engine charges the whole hold.
- **Collecting** a long job a previous run submitted bills nothing new ([protocol.md](protocol.md) §6.1 step 5). A job that ends without a result in the run that submitted it bills what its adapter reports for the ended job (`Ended { cost }`, §7), else its whole hold. As for a refusal (§4.3), `Some(0)` is never inferred: an ended job is $0 only where its provider's row in §9 says so, and §9 records why.
- **A job the submitting run stopped waiting for keeps its whole hold.** That run settles the hold in full when the job is `Unreachable` (§7), since the job may still be running. A later run that finds the job ended, at whatever cost, bills nothing new and changes nothing already booked.

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
- **`Ended { cost }`** carries what the ended job costs: the provider's own figure where §9 reads one for a job that ended that way; `Some(0)` only where §9 says the provider does not bill such a job and it reports no figure; else `None`. The run that submitted the job settles its hold at that cost, else in full (§6). A result that is unusable came from a job the provider finished and billed, so it costs what the provider reported for that job, as an answer would, and `None` when the provider reported nothing that can be read.
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
  - An `Ended` after the result read (a result without an https video, or a download that ends the job) comes from a job fal finished, so it costs that read's `usage.cost`, as an answer would (§7). Every other `Ended` costs `None`, and so does a result read without a cost: the run that submitted the job then charges its whole hold.
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
  - Refused before sending, in this order: no views (`a mesh call needs its views, by name`); no `front`, or a view other than front, back, left or right (`a multiview task takes front and any of back, left, right`, followed by `; not <names>` listing the unknown names, sorted); `front` alone (`a multiview task takes front and at least one of back, left, right`); a `face_limit` outside 48 to 25 000 (`face_limit is 48 to 25000`); then, in the order front, back, left, right, a view that is not PNG or JPEG (`the <view> view is <kind>; Tripo takes PNG or JPEG`) or has no bytes (§5 step 5).
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
  - 400, 401, 403, 404 and 422: `Failed { cost: None, retryable: false }`. Tripo's [billing documentation](https://developers.tripo3d.com/en/docs/billing) says credits are frozen when a task is created, and its [task lifecycle](https://developers.tripo3d.com/en/docs/task-lifecycle) says a submitted request returns a `task_id`. A refused POST returns none. But Tripo does not document a refusal as unbilled, and `Some(0)` is never inferred (§4.3);
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
  - an `Ended` for `failed` or `cancelled` costs the `credits_consumed` of that status read, converted as under **Cost**, and `Some(0)` when the read has none (absent or `null`). A figure that cannot be read costs `None`. Tripo's [billing documentation](https://developers.tripo3d.com/en/docs/billing) says `Failed and cancelled tasks are not charged`, and its [task lifecycle](https://developers.tripo3d.com/en/docs/task-lifecycle) says the frozen credits of a failed or cancelled task are released. A figure Tripo reports still wins over that rule (§6). An `Ended` for an unusable output (the URL scan, a host, a file's kind, or no model an answer can keep) comes from a task that succeeded, so it costs that success read's `credits_consumed`, as an answer would (**Cost**). `banned` and `expired` cost `None`, whatever the read reports: Tripo documents nothing for them;
  - an unknown status, another task's answer, and a download that fails or is over the cap are `Unreachable`;
  - a 401, a 403, a 3xx or a transport refusal on a status read ends polling at once as `Unreachable`; other failed reads (404 and 429 included) are polls that saw nothing.
- **Cost:** `credits_consumed` of the finished task, a number or a decimal string, × USD 0.01, rounded up (§6), whether its output answers or is unusable; `None` when it is absent or cannot be read. The check's credits are not counted. A task that ended `failed` or `cancelled` costs its own read's `credits_consumed` the same way, or $0 when that read reports none (above). A refused paid POST, and a `banned` or `expired` task, are charged the whole hold. These costs are booked by the run that submitted the task, when it sees the task end. A task still running when that run stops waiting keeps the whole hold that run charged, and a later run that finds it ended bills nothing new (§6).

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

### 9.6 Codex

- **Capabilities:** `image.generate` and `image.edit`, on the user's own Codex: the `codex` command, signed in with the user's ChatGPT account. Contract `adapter`: `fx-codex-image-v1`, behaviour `"2"`, the string. The behaviour names the turn's instructions and the conversation's settings below; a change to either is a new behaviour. Behaviour `"1"` named Codex's image skill and took no pictures; it is refused (§5 step 1).
- **The model is Codex's.** The route's model, `imagegen`, names Codex's image tool (`image_gen.imagegen`), not a model: Codex chooses the model and the quality, and FX neither picks nor records them, so the route's cache keys outlive a change of either. `independent_of` cannot reason about this route.
- **No key, no transport.** The adapter starts `codex app-server` and speaks its JSON-RPC, one JSON message per line on stdin and stdout. Codex owns the sign-in, the model, the network and the usage: FX never reads Codex's credential files, and books nothing in dollars. Codex retries its own requests to OpenAI within a turn; that is Codex's, and FX never resends a Codex call (below).
- **The program:** `GRIDA_FX_TOOL_CODEX` when set and not blank, made absolute against the working directory, and then that program only: none when it names no executable file. Else the first `codex` in an absolute `PATH` entry (empty and relative entries are skipped), else `/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex`. Each must be an executable file. A live invocation looks for it once, and not at all with the network off; `grida-fx doctor` looks for it too, only to report it.
- **The environment** is FX's, without the eleven variables of §3 and without `CODEX_API_KEY`, `OPENAI_ORG_ID`, `OPENAI_ORGANIZATION`, `OPENAI_PROJECT` and `OPENAI_PROJECT_ID`. Codex's stderr is dropped.
- **The user's tools are off, and so are Codex's own.** Every Codex FX starts gets `-c notify=[]`, `-c features.memories=false`, `-c features.hooks=false`, `-c features.apps=false` and `-c features.plugins=false`; then `-c features.<name>=false` for `shell_tool`, `unified_exec`, `multi_agent`, `browser_use`, `computer_use`, `in_app_browser`, `view_image`, `goals`, `sleep_tool`, `tool_suggest` and `skill_search`, and `-c web_search="disabled"`, so a turn is offered its image tool and little else (Codex ignores a feature it does not know, and a turn's context is about half as long); and `-c mcp_servers.<name>.enabled=false` for each enabled server `codex mcp list --json` reports under those settings (`<name>` bare when it is letters, digits, `_` and `-`, else a TOML basic string). A server only a plugin defines is not named: Codex refuses a setting for it, and `features.plugins=false` already turns it off. A list Codex will not give within 30 s is a refusal: `Codex's MCP servers could not be listed, so they cannot be turned off`.
- **The runtime.** One `codex app-server` serves an invocation: started on its first Codex call, kept for the others, and shut down with the run (§1). Starting it: the MCP list below, then Codex in a scratch folder of its own, in a process group of its own (its MCP servers and helpers join it), then `initialize`, the `initialized` notification and `account/read`. A Codex that cannot draw on the user's plan (the refusals below) is refused for every call of the invocation, and is not asked again; a Codex that exits is replaced by the next call. Shutting down closes Codex's stdin, gives it 2 s to exit, sends `SIGTERM` to the group and 2 s more, then kills the group; a command that exits on an interrupt kills the group at once. Process groups are Unix's: elsewhere only Codex itself is ended.
- **Pipes.** One task writes Codex's stdin, and one reads its stdout: each response goes to the request waiting for it, each notification to the thread it names (a thread's notifications open with its `thread/start` response, so none is missed), and a request Codex sends (an approval, a question) is answered with error `-32601`: FX grants nothing.
- **Pictures.** A call's input pictures are `image` (an edit's), then `references`, in order: at most 5, Codex's own limit (its image tool draws nothing for more). Each is PNG, JPEG or WebP. FX copies each one's store copy into the call's scratch folder as `picture-<n>.<png|jpg|webp>` (`n` from 1, two digits), and names them in the turn's text by absolute path, in order, as the image tool's `referenced_image_paths`: Codex reads them through its read-only sandbox, at their own resolution, and sends them to its image endpoint for edits. Nothing is attached to the turn: a picture attached to the conversation is, in Codex's own words, a best-effort reference. A mask is refused: Codex's image tool takes none.
- **Shapes.** Codex's image tool takes no size: its images come back about 1.57 megapixels. Without input pictures their shape follows the prompt's wording, between 1:3 and 3:1 (asked for about 4:1, about 1:3 and 16:9, it drew 2172x724, 724x2172 and 1672x941; asked for 1024x1024 it drew 1254x1254). With them, an image takes about the first picture's shape unless the prompt asks for another (a 1000x400 picture, edited, came back 1984x793).
- **Limits:** 60 s for Codex to accept the turn (starting the runtime too, on the invocation's first call), and 600 s for the whole send; a message from Codex is at most 64 MiB, refused as soon as it passes that.
- **Refusals (§5), in this order:**
  - the route (§5 step 1) and the request (§5 step 2);
  - a blank `prompt`: `an image call needs its prompt as text`;
  - a `size` other than `auto`: `Codex image generation takes no exact size: use size auto, and fx/image.resize@1 after it` (Shapes, above: an exact size would only fail the check);
  - a `background` other than `auto`, `opaque` or `transparent`: `background must be auto, opaque or transparent`;
  - a `mask`: `Codex image edits take no mask`;
  - more than 5 pictures: `Codex takes at most 5 input pictures`;
  - a picture the request does not carry, or whose store copy is empty or unreadable: `<member> has no bytes to send`; one of another kind: `<member> is <kind>; Codex takes PNG, JPEG or WebP` (`<member>` is `image` or `references[<i>]`);
  - **the usage limit**, once a call of the invocation found it reached (an image item's failure `usageLimitExceeded`, or a turn's `codexErrorInfo`): `the Codex usage limit is reached; it resets at unix time <resetsAt>, and no Codex call starts before then`, until that time; without a time, `the Codex usage limit is reached, and no Codex call starts again in this run`. Nothing is started for such a call, so a run with many Codex steps stops spending turns on a plan that cannot draw;
  - no program: `Codex is not installed: install it with npm install -g @openai/codex and sign in with codex login, or set GRIDA_FX_TOOL_CODEX`; with the network off: `Codex was not started: GRIDA_FX_NETWORK is off`. A program that cannot be started: `Codex image generation cannot start codex: <reason>`;
  - the MCP list above;
  - **an account that does not draw on the user's plan**, from `account/read` when the runtime starts: no account, `Codex is not signed in: run codex login and sign in with ChatGPT`; an API key, `Codex is signed in with an API key, which bills an API account: run codex login and sign in with ChatGPT`; any other, `Codex is signed in with <type>, not ChatGPT: run codex login and sign in with ChatGPT`;
  - a thread whose `modelProvider` is not `openai`: `Codex uses the model provider <provider>, not OpenAI: image generation needs Codex signed in with ChatGPT`.
- **One send:**
  1. the runtime's Codex, started if need be (above);
  2. a scratch folder for this call alone, holding only its pictures (above), which FX removes after it; then `thread/start` in it, with `sandbox: read-only`, `approvalPolicy: never` and `ephemeral: true`;
  3. `turn/start` with `effort: low` and `serviceTierForTurn: default` (the user's own settings could spend their plan faster for nothing an image needs), and one text input: `Call the image_gen.imagegen tool exactly once. Pass the text between the <prompt> tags as its prompt, exactly as written. Set transparent_background to true.` for `background: transparent`, else the same ending `false.` (Codex then draws an opaque image, so `auto` is opaque); then, with pictures, ` Set referenced_image_paths to these input images, in this order: ` and each path between backticks, separated by `, `, and `.`; then ` Do not run commands, read or edit files, use any other tool or ask questions. When the image is made, reply with one short sentence.`; a blank line; then `<prompt>`, the prompt verbatim, and `</prompt>`, each on its own line. The skill mention `$imagegen` is not used: it brings a skill written for a person's project, twice as long as the tool's own description;
  4. the thread's notifications of this turn (`turnId`, `turn.id`), until the first `imageGeneration` item that completed with an image, or `turn/completed`; `thread/tokenUsage/updated` is read on the way.

  A send that is dropped (its run was cancelled) or runs out of time asks Codex to stop its turn (`turn/interrupt`); the runtime goes on. One dropped before Codex answered its `turn/start` cannot name the turn yet: the runtime's reader, which receives that answer, interrupts the turn it names.
- **Answer.** The first `imageGeneration` item that completed: its `result` must be strict base64 of the image. FX then asks Codex to stop the rest of the turn (`turn/interrupt`) and reads its token usage until the turn ends, or 3 s. The answer is file `image` (its kind sniffed from the bytes; the check wants `image/png`) and `data: null`. Cost: `None`, since Codex reports none: the route's hold, $0, is what is charged, and the record says the cost was not reported. Usage (§6): billing `plan`, `runtime` `codex <version>` (from `initialize`'s `userAgent`), `agent_model` (the thread's `model`), and the turn's total `input_tokens`, `cached_input_tokens` and `output_tokens`, each when Codex gave it. A version or model is kept only when it is letters, digits, `.`, `-`, `_` and `+` (a version) at most 40, or letters, digits, `.`, `-` and `_` (a model) at most 60.
- **Outcomes.** One error is provably not received: `-32001` (Codex's app-server is overloaded and rejected the request on arrival) in answer to any of FX's requests is `NotReceived` (§4.1), `Codex image generation was not started: Codex is overloaded (error -32001)`, and the engine may send the call again. Every other failure is `Failed { cost: None, retryable: false }`, and that call is never sent again by the engine. Before `turn/start` is written, a failure is most likely to repeat (a Codex that will not start, a setting it refuses); once it is written, an image may already have been made and counted against the user's plan. The reason starts `Codex image generation was not started: ` before `turn/start` is written, and `Codex image generation failed: ` after, followed by:
  - `Codex exited`, `Codex's output is unreadable`, `Codex sent a line that is not JSON`, `Codex sent a message over 64 MiB`, `Codex could not read a request`, `Codex started no thread`, `Codex has no pipes`, `the command is stopping`;
  - `Codex refused <method> (error <code>)` for an error response;
  - `Codex did not start the turn within <n> s`, or `Codex did not finish within <n> s` (FX first asks Codex to stop the turn);
  - for an image item that failed: `the Codex usage limit is reached; it resets at unix time <resetsAt>` (without the time when Codex gives none), or `Codex's image generation failed (<type>)`; one that did not complete: `Codex's image generation ended <status>`; a `result` that is not base64: `Codex's image is not valid base64`. A later good image of the same turn wins over these;
  - for a turn that completed without an image: by its error's `codexErrorInfo`, `the Codex usage limit is reached` (`usageLimitExceeded`), `Codex is not signed in: run codex login and sign in with ChatGPT` (`unauthorized`), or `Codex's turn failed (<kind>)`; an error without one, `Codex's turn failed`; no error, `Codex answered without an image`, or `Codex's turn ended <status> without an image`.

  `<type>`, `<kind>`, `<status>` and `<provider>` are Codex's words only when they are letters, digits and `_`, at most 40 of them, else `unknown`. Codex's own messages are never shown: they can name private paths (§8).
- **Check:** the image checks of [capabilities.md](capabilities.md) §2. An answer that fails them is a billed attempt like any other (§4.4): the engine may make a new attempt, and each is a new image on the user's plan.
- **Reasons:** the label is `Codex image generation`.

### 9.7 Grida Gateway

- **Capabilities:** GG (`gg`) is Grida's own provider: one organization key serves text, images, video and 3D through one OpenAI-compatible API, billed to the organization's Grida AI credits.
  - `image.generate` (`POST {base}/images/generations`, JSON) and `image.edit` (`POST {base}/images/edits`, multipart). Contract `adapter`: `gg-image`, behaviour `"1"`, the string.
  - `structured.generate` (`POST {base}/chat/completions`, a strict `json_schema` response format). Contract `adapter`: `gg-structured`, behaviour `1`.
  - `agent.turn` (`POST {base}/chat/completions`, strict function tools). Contract `adapter`: `gg-tool-loop`, behaviour `1`.
  - `video.generate` (`POST {base}/videos`, a long job read at `GET {base}/jobs/<id>`). Contract `adapter`: `gg-video`, behaviour `1`.
  - `mesh.generate` from views (`POST {base}/endpoints/<endpoint>`, multipart, a long job). Contract `adapter`: `gg-mesh`, behaviour `1`.
  - `mesh.rig` (`POST {base}/endpoints/<endpoint>`, multipart, a long job). Contract `adapter`: `gg-rig`, behaviour `1`.

  GG publishes every model's exact request schema and refuses a request that does not match it before any paid work, at no cost. So the adapters check FX's own shapes and leave each model's limits (sizes, counts, durations, tiers, face limits, rig types, which members a model takes) to GG.
- **Files go with the request.** Pictures and models travel in the request itself: inline for text, as `image[]` parts for an image edit, and for a job as the parts of a `multipart/form-data` submit, beside its JSON as the text part `input`. GG keeps none of them, and FX never hosts a file for GG to fetch.
- **Not served:** a last frame and masked edits: GG's API has neither.
- **Credential:** `authorization: Bearer <GG_API_KEY>`.
- **Idempotency.** Every paid request carries `idempotency-key: fx-<session>-<call>-<attempt>`: `<session>` is 16 hexadecimal characters drawn from the operating system's random source for each invocation's adapters, `<call>` the first 16 letters and digits of the call key, and `<attempt>` the attempt's number. Every send of one attempt (§4.1, `NotReceived`) carries the same key, so GG never takes one attempt twice; every new attempt, in this invocation or a later one, carries a new key, and is new work. The one exception: a 429 whose error says both `request_not_admitted` and `retry_with_new_idempotency_key` (`true`) renews the attempt's key, `.<n>` appended for the `n`th renewal, so the engine's resend is an identity GG may admit. GG says nothing was dispatched or held, and keeps the refused key's outcome.
- **Route contract policy.** A member other than those below is refused as `<route> is not a <capability> route this adapter serves: the contract takes no member <m>`, and so is a bad value, with its sentence after the colon.
  - `gg-image`: `quality`, the model's quality setting, sent as given (`quality is text`).
  - `gg-structured` and `gg-tool-loop`: `image_detail`, applied to every picture (`image_detail is one of auto, low, high`); `reasoning_effort`, sent as given (`reasoning_effort is one of none, minimal, low, medium, high, xhigh`); and, for `gg-structured` only, `pictures: unchanged` (`pictures is "unchanged" or absent`), as on OpenRouter (§9.2).
  - `gg-video`: no other member.
  - `gg-mesh` and `gg-rig`: `endpoint`, the id of GG's specialized endpoint the route submits to, such as `three-d.tripo.p2.multiview` (`endpoint is one of GG's endpoint ids`: one path segment matching `^[A-Za-z0-9][A-Za-z0-9_.-]{0,159}$`). It is required (`the contract names no GG endpoint`): GG's endpoint ids are opaque, and a model does not name one, so a route these adapters serve always says where it goes. `gg-rig` also takes `check_endpoint`, the free riggability check to run first (`check_endpoint is one of GG's endpoint ids`).
- **Limits:**

  | Capability | Deadline | Response cap |
  |---|---|---|
  | Images | 630 s (GG answers within 600 s) | 64 MiB |
  | Structured | 900 s | 16 MiB |
  | Agent turns | 600 s | 16 MiB |
  | Job submit, job read | 120 s, 60 s | 4 MiB |
  | Job collect | 1800 s, polled every 5 s | clip download 512 MiB; model download 150 000 000 bytes |
  | Billing read | 30 s | 1 MiB |

- **Images.**
  - The wire, in this order: `model`, `prompt` verbatim, `n: 1`, `size` when the request gives `<W>x<H>` (`auto` and none are left out: the model chooses), the contract's `quality`, and `background` when the request gives `opaque` or `transparent` (`auto` is the default, and a member some models do not take). An edit sends them as text fields, then each input picture (`image` first, then `references`) as a part `image[]`, named `reference-NN.<png|jpg|webp>` with `NN` from `01`.
  - Refused before sending, in this order: a blank `prompt` (`an image call needs its prompt as text`); a `background` other than `auto`, `opaque` or `transparent` (`background must be auto, opaque or transparent`); a `size` that is not `auto` or `<W>x<H>` (`GG image size must be auto or WIDTHxHEIGHT`); references on a generate (`GG image generation takes no references`); a mask (`GG image edits take no mask`); more than 16 input pictures (`GG image edits support at most 16 input images`); an input whose kind is not PNG, JPEG or WebP (`<member> is <kind>; GG takes PNG, JPEG or WebP`), or whose bytes do not decode as their kind (`<member> is not a decodable <kind>`). `<member>` is `image` or `references[<i>]`, counted from 0.
  - **Answer:** `data[0].b64_json`, the only item, strict base64; else the send is a structural failure. The answer is file `image`, of the kind its bytes show, and `data: null`. A declared `media_type` is not read: the check wants PNG.
  - **Check:** the image checks of [capabilities.md](capabilities.md) §2.
- **Structured output and agent turns** are OpenRouter's (§9.2) in every message, schema, tool, picture, refusal and answer, with these differences:
  - The body is `{model, messages, response_format, max_completion_tokens, reasoning_effort?}` for structured output and `{model, messages, tools, tool_choice, max_completion_tokens?, reasoning_effort?}` for a turn: nothing GG would have to route.
  - A body over 20 MiB, GG's limit, is refused before sending: `request body exceeds 20 MiB`.
  - The answer's usage (§6) is the completion's `usage`: `prompt_tokens`, `prompt_tokens_details.cached_tokens` and `completion_tokens`, each when given. Its `cost` is not GG's charge, and is not read.
- **Jobs** (video, 3D models, rigs) share one submit and one collect.
  - **Submit:** the paid POST, once. A 2xx whose `id` matches `^[A-Za-z0-9][A-Za-z0-9_.-]{0,159}$` becomes the handle `{"job_id": <id>}`; any other 2xx is `Uncertain`. A 3xx and a 5xx are `Uncertain`; 408 and 429 (but `key_budget_exceeded`) are `NotReceived`, as for a plain request (below), and a 429 that asks for a new key renews it; any other 4xx in GG's error envelope is `Failed { cost: Some(0), retryable: false }`, and without the envelope `Failed { cost: None, retryable: false }`. An `Uncertain` reason names GG's request id and the attempt's `Idempotency-Key`, so a person can find the job in GG's console.
  - **Collect:** the job is read every 5 s. `status` is `accepted`, `running`, `unknown`, `succeeded` or `failed`, and `result_status` is `pending`, `available`, `unavailable` or `expired`; any other is `Unreachable`. A job is waited for while its result is `pending`, or `unavailable` or `expired` before `succeeded` or `failed`. A `succeeded` or `failed` job whose result is `unavailable` or `expired` is `Ended`, its reason naming GG's error `code` when that is a plain name. An `available` result's file URL must be `https` with a host, no userinfo and no fragment, else the job is `Ended`. A read that fails (a transport failure, 408, 429, a 5xx, a body that is not a JSON object) saw nothing, and polling goes on; a 410 (`job_expired`) is `Ended`; any other status, and an answer about another job, is `Unreachable`.
  - **Download:** once, on the download lane, at the URL exactly as GG gave it. A transport failure, a 3xx, 401, 403, 408, 429 or a 5xx is `Unreachable` (a later collect reads the job and downloads again); another 4xx, an empty body, one over the cap, or bytes of the wrong kind is `Ended`.
- **Video.**
  - The submit body: `model`, `prompt`, `duration` (an integer when whole), `aspectRatio` from `aspect_ratio`, and `resolution` when it is `<W>x<H>`; a tier name (`720p`, `4k`) goes as `providerOptions: {gg: {resolutionTier: <name>}}` instead. With a `first_frame`, the submit is multipart: that JSON as `input`, then the frame as the part `image`, named `first-frame.<png|jpg|webp>`.
  - Refused before sending, after the route, the request and the key: a blank `prompt` (`a clip needs its prompt as text`); a `last_frame` (`GG video takes no last frame`); a `resolution` that is neither a tier name (letters and digits) nor `<W>x<H>` (`resolution is a tier name, such as 720p, or WIDTHxHEIGHT`); an `aspect_ratio` that is not `W:H` (`aspect_ratio is W:H, such as 16:9`); a first frame that is not PNG, JPEG or WebP (`first_frame is <kind>; GG takes PNG, JPEG or WebP`) or has no bytes (§5 step 5).
  - **Answer:** `result.videos[0]`, an MP4 with a video track, as file `video` (`video/mp4`), `data: {"facts": {"width", "height", "duration_seconds", "fps"}}`.
  - **Check:** the clip runs within 0.5 s of a requested `duration`, and has the exact size of a requested `<W>x<H>`.
- **3D models** (`gg-mesh`).
  - The submit's `input` is `{texture?, pbr?, face_limit?, quad?}`, each as the request gives it, and `quad` only when `true`: GG's defaults apply to what is absent. Each view is a part `images.<view>`, named `<view>.<png|jpg>`, in the order front, left, back, right.
  - Refused before sending, after the route, the request and the key: no views (`a mesh call needs its views, by name`); no `front`, or a view other than front, left, back or right (`a multiview job takes front and any of left, back, right`, followed by `; not <names>` listing the unknown names, sorted); `front` alone (`a multiview job takes front and at least one of left, back, right`); then, in the order front, left, back, right, a view that is not PNG or JPEG (`the <view> view is <kind>; GG takes PNG or JPEG`) or has no bytes (§5 step 5).
  - **Answer:** `result.glb`, a GLB (`glTF` first), as file `model` (`model/gltf-binary`), `data: {"facts": {"model_kind": "model/gltf-binary"}}`; any other file ends the job (`GG output model is not a GLB file`).
- **Rigs** (`gg-rig`).
  - The submit's `input` is `{rig_type, spec}`: the request's `rig_type`, else `biped`, and its `skeleton`, else `mixamo` (an empty text counts as none). The model is the part `mesh`, named `unrigged.glb`.
  - Refused before sending, after the route, the request and the key: a model that is not `model/gltf-binary` (`GG rigs a GLB, not <kind>`) or has no bytes (§5 step 5).
  - **The free check.** With a `check_endpoint`, the submit begins with a free phase (§7), as Tripo's does (§9.4): the model goes to the check endpoint (`input` `{}`, the part `mesh`), under the attempt's `Idempotency-Key` with `-check` appended, and its job is read every 5 s for at most 600 s. Its result must hold a boolean `riggable`; its `rig_type` is kept only when it is a string matching `^[A-Za-z0-9_.:-]{1,96}$`, else `null`. A doubted model (`riggable` not true, or `rig_type` not the requested one) is refused unless `allow_negative_check` is true: `GG's rig check doubts this model (riggable=<b>, rig_type=<t>)`. A check submit refused with a 4xx other than 408 and 429, or a check job that ends without a result, is `Refused`; any other failure (a transport failure, 408, 429, a 5xx, a 2xx without a job, a check still running at its deadline, a `riggable` that is not a boolean) is `NotReceived`, and the engine sends the whole submit again, which GG answers with the same check job. The handle is then `{"job_id", "check": {"job_id", "riggable", "rig_type"}, "advisory_override"}`; without a check, `{"job_id"}`.
  - **Answer:** `result.glb`, a GLB, as file `model` (`model/gltf-binary`), `data: {"facts": {"riggable", "checked_rig_type", "advisory_override"}}` from the handle: `null`, `null` and `false` when no check ran.
- **Cost.** GG's money is exact decimal mills, `^(0|[1-9][0-9]{0,19})(\.[0-9]{1,18})?$`, 1 000 mills to USD 1, rounded up to whole micro-dollars (§6); any other text is no figure.
  - **An image or text answer** costs GG's charge once GG has settled it: `x-gg-charged-mills` when `x-gg-billing-status` is `settled`. Otherwise, when `x-gg-billing-url` is exactly `/v1/requests/<id>/billing`, with an `<id>` that matches `^[A-Za-z0-9][A-Za-z0-9_.-]{0,95}$`, the adapter reads `GET {base}/requests/<id>/billing` after 1 s and then after each wait, doubled, up to 8 s, for at most 30 s in all: a `request_billing` whose `billing.status` is `settled` gives `charged_mills`. A read that fails in a way the next may not (a transport failure, 408, 429, a 5xx, a body that is not a JSON object, a status `reserved` or `pending`) waits for the next; any other status, a read about another request, an undocumented `billing.status` or a settled charge that is not GG's money ends the wait. A billing URL in any other shape is not read.
  - **A failure GG accepted** (every `Failed { cost: None }` of the statuses below, and a 2xx that does not become an answer) costs GG's settled charge, read the same way when the response names its billing, else `None`. GG keeps the billing of every request it accepted, failed ones included, and settles a provider's refusal at what the provider charged: often $0.
  - **A reservation is not a charge.** `x-gg-reserved-mills`, and a job's `billing.authorized_mills`, are what GG holds for the accepted request until it settles, and are never read as its cost. A charge that has not settled when the wait ends is unknown: the answer reports no cost, and the engine charges the whole hold (§6). `charged_mills: "0"` is a known zero.
  - **A job's answer** costs `billing.charged_mills` once the job's `billing.status` is `settled`. When the file is in hand and billing has not settled, the job is read again every 5 s for at most 60 s more; then the answer reports no cost. An `Ended` job costs its settled charge, else `None`.
  - Waiting for a charge is the one sleep, besides a long job's polling, that §1 allows an adapter: the exchange is over, and the wait only prices it.
- **Statuses (overrides of §4.3)** of an image or text request; a job's submit follows its own row above:
  - **3xx:** `Failed { cost: None, retryable: false }`: `<label> was redirected (HTTP <n>); check GG_BASE_URL`.
  - **408 and 429:** `NotReceived`, with the wait the response asks for. GG admitted nothing. A 429 that asks for a new key renews it first (above).
  - **429 `key_budget_exceeded`:** `Failed { cost: Some(0), retryable: false }`. The key's own spending cap is reached.
  - **409 `already_submitted`:** `Failed { cost: None, retryable: false }`. GG took the original and keeps no bytes to replay.
  - **409 `idempotency_conflict` and `price_revision_changed`:** `Failed { cost: Some(0), retryable: false }`. Nothing new was dispatched.
  - **Any other 409, and a 410:** `Failed { cost: None, retryable: false }`.
  - **Any other 4xx in GG's error envelope (`{"error": {...}}`):** `Failed { cost: Some(0), retryable: false }`. GG documents that an authentication, validation or credit refusal before admission creates no billable request. Without the envelope, the cost is `None`.
  - **502 `upstream_rejected`:** `Failed { cost: None, retryable: false }`. The model's provider refused the request as given.
  - **Any other 5xx:** `Failed { cost: None, retryable: true }`. GG may have done the work (a 503 `temporarily_unavailable` on an image is a wait that expired, not a cancellation), so the attempt is billed, at GG's settled charge when it gives one (above), and a new attempt is new work under a new key.
- **Reasons:** the labels are `GG image generation`, `GG structured generation`, `GG tool loop`, `GG video submission`, `GG video job`, `GG output video download`, `GG 3D model submission`, `GG 3D model job`, `GG rig submission`, `GG rig job`, `GG rig check submission`, `GG rig check job` and `GG output model download`, each with the safe detail of the error envelope and ` (request <id>)` when the response's `x-request-id`, else its error's `request_id`, is one of GG's ids. Then a hint: `check GG_API_KEY` (401), `add Grida AI credits at https://console.grida.co` (402 `insufficient_credit`), `GG reserves more for this call than its ceiling allows` (402 `cost_limit_exceeded`), `check the key and organization at https://console.grida.co` (403, and 429 `key_budget_exceeded`), or `nothing was admitted; it is sent again under a new Idempotency-Key` (a 429 that renews the key).

### 9.8 Grok Build

- **Capabilities:** `image.generate` and `image.edit`, on the user's own Grok Build: the `grok` command, signed in with the user's Grok account, whose subscription includes image generation. Contract `adapter`: `fx-grok-image-v1`, behaviour `"1"`, the string. The behaviour names the prompt's instructions, the session's agent and the settings below; a change to any is a new behaviour.
- **The model is Grok Build's.** The route's model, `imagine`, names Grok Build's image tools (`image_gen`, `image_edit`), not a model: Grok Build chooses the image model and the agent model, and FX does not pick them; the call record keeps the agent model only (§6). So the route's cache keys outlive a change of either. `independent_of` cannot reason about this route.
- **No key, no transport.** The adapter starts `grok agent --no-leader stdio` and speaks the Agent Client Protocol (ACP v1) to it: JSON-RPC 2.0, one JSON message per line on stdin and stdout. `--no-leader` keeps it from joining a Grok Build the user already runs. Grok Build owns the sign-in, the models, the network and the usage: FX never reads Grok Build's credential files, and books nothing in dollars. FX never resends a Grok Build call (below).
- **The program:** `GRIDA_FX_TOOL_GROK` when set and not blank, made absolute against the working directory, and then that program only: none when it names no executable file. Else the first `grok` in an absolute `PATH` entry (empty and relative entries are skipped), else `$HOME/.grok/bin/grok`, where Grok Build's installer puts it. Never `agent`, a name other programs also install. Each must be an executable file. A live invocation looks for it once, and not at all with the network off; `grida-fx doctor` looks for it too, only to report it.
- **The environment** is FX's, without the eleven variables of §3 and without `XAI_API_KEY` (an xAI API key bills an API account) and `GROK_CONFIG_PATH`; with `GROK_DISABLE_API_KEY_AUTH=1`, `GROK_DISABLE_AUTOUPDATER=1`, `GROK_MANAGED_MCPS_ENABLED=false`, `GROK_<SOURCE>_<KIND>_ENABLED=false` for each source `CLAUDE`, `CURSOR` and `CODEX` and each kind `SKILLS`, `RULES`, `AGENTS`, `MCPS`, `HOOKS` and `SESSIONS` (nothing is imported from those programs' settings), and `GROK_CONFIG`, a configuration overlay that turns image generation on and `telemetry`, `video_gen`, `web_fetch`, `lsp_tools`, `write_file`, `backend_tools`, `codebase_indexing`, `ask_user_question`, `session_recap` and `turn_summary` off. Grok Build's stderr is dropped.
- **The runtime.** One Grok Build serves an invocation: started on its first Grok call, kept for the others, and shut down with the run (§1). Starting it: Grok Build in a scratch folder of its own, in a process group of its own (whatever it starts joins it), then `initialize` (protocol version 1, and a client that offers no file system and no terminal), whose `_meta.agentVersion` must be 1.0.52 or later, the version this behaviour was verified on; then `authenticate` with the method `cached_token`, the sign-in `grok login` caches. A Grok Build that cannot draw on the user's subscription (the refusals below) is refused for every call of the invocation, and is not asked again; a Grok Build that exits is replaced by the next call. Shutting down is as for Codex (§9.6).
- **Sessions.** Each call has a session of its own, in the runtime's scratch folder: its pictures are copied into a folder of the call's own inside it. One working folder keeps what Grok Build records per working folder, such as its prompt history, to one per invocation. A session's agent is FX's (`session/new`'s `_meta.agentProfile`): `{"name": "grida-fx-image", "description": "Draws one image with Grok Build's image tools", "tools": ["image_gen", "image_edit"], "disallowedTools": ["search_tool", "use_tool"]}`, so the model is offered Grok Build's two image tools and nothing else: no shell, no files, no subagents, no web, no MCP tools, whatever the user's own permission settings say. The session asks before every tool call (`_meta.yoloMode: false`), whatever the user's own permission mode, so FX's answers (below) decide each one. `mcpServers` is empty. The session's reasoning effort is set to `low` (`session/set_config_option`); a Grok Build that refuses it draws at its own.
- **Pipes.** One task writes Grok Build's stdin, and one reads its stdout: each response goes to the request waiting for it, each notification that names a session to that session (a session's notifications open with its `session/new` response, so none is missed). Grok Build's requests are answered: `session/request_permission` with `allow_once` for `image_gen` and `image_edit` (named by the tool call's `_meta["x.ai/tool"].name`, or by the session's earlier `tool_call` of the same id) and `reject_once` for anything else, never `allow_always`; any other request with error `-32601`. FX grants nothing else.
- **Pictures.** A call's input pictures are `image` (an edit's), then `references`, in order: at most 3, the most Grok Build's image edit model takes (it answers more with HTTP 400, though xAI's own image API documents 5). Each is PNG or JPEG. FX copies each into the call's folder as `picture-<n>.<png|jpg>` (`n` from 1, two digits), and names them in the prompt by absolute path, in order, as `image_edit`'s `image`. A call with pictures uses `image_edit`, one without uses `image_gen`. A mask is refused: Grok Build's tools take none.
- **Shapes.** Grok Build's tools take an aspect ratio and no size, and FX leaves the aspect ratio unset: Grok Build picks the shape, with pictures too (two square pictures came back 1024x1024, three 864x1152). It draws about one megapixel (1:1 was 1024x1024; asked for 16:9 and 9:16, it drew 1280x720 and 720x1280; left to choose, 864x1152).
- **Limits:** 60 s for Grok Build to take the prompt (starting the runtime too, on the invocation's first call), and 300 s for the whole send; a message from Grok Build is at most 64 MiB, refused as soon as it passes that.
- **Refusals (§5), in this order:**
  - the route (§5 step 1) and the request (§5 step 2);
  - a blank `prompt`: `an image call needs its prompt as text`;
  - a `size` other than `auto`: `Grok Build image generation takes no exact size: use size auto, and fx/image.resize@1 after it`;
  - `background: transparent`: `Grok Build draws opaque images: it takes no transparent background`; any other value but `auto` and `opaque`: `background must be auto, opaque or transparent`;
  - a `mask`: `Grok Build image edits take no mask`;
  - more than 3 pictures: `Grok Build takes at most 3 input pictures`;
  - a picture the request does not carry, or whose store copy is empty or unreadable: `<member> has no bytes to send`; one of another kind: `<member> is <kind>; Grok Build takes PNG or JPEG`;
  - once a call of the invocation found that the account's image tools need another subscription: `Grok Build's image tools need a SuperGrok subscription on the signed-in account`;
  - no program: `Grok Build is not installed: install it from https://x.ai/cli and sign in with grok login, or set GRIDA_FX_TOOL_GROK`; with the network off: `Grok Build was not started: GRIDA_FX_NETWORK is off`. A program that cannot be started: `Grok Build image generation cannot start grok: <reason>`;
  - an older Grok Build: `Grok Build <version> is older than 1.0.52: run grok update`; one that names no version: `Grok Build did not name its version: run grok update to 1.0.52 or later`;
  - **an account that does not draw on a subscription**: no `cached_token` method, or `authenticate` refused, `Grok Build is not signed in: run grok login`; an account `authenticate` says is billed through the API (`_meta.backend_billed: true`), `Grok Build bills this account through the xAI API, not a Grok subscription: sign in with grok login`. Nothing else of the account is read or kept.
- **One send:**
  1. the runtime's Grok Build, started if need be (above);
  2. the call's folder holding its pictures (above), which FX removes after it; then `session/new` with FX's agent, and the effort;
  3. `session/prompt` with one text block: `Call the image_gen tool exactly once.` without pictures; with them, `Call the image_edit tool exactly once. Set image to these input images, in this order: ` and each path between backticks, separated by `, `, and `.`; then ` Pass the text between the <prompt> tags as its prompt, exactly as written, and leave aspect_ratio unset. Do not use any other tool or ask questions. When the image is made, reply with one short sentence.`; a blank line; then `<prompt>`, the prompt verbatim, and `</prompt>`, each on its own line;
  4. the session's `tool_call` and `tool_call_update` notifications, until an image tool call that completed with an image, or the prompt's answer. A tool call of any other tool ends the call.

  The call then ends its session: a turn that has not ended is cancelled (`session/cancel`), and the session is deleted (`_x.ai/session/delete`, waiting 3 s at most), so neither the session nor its image stays in the user's Grok Build. A send that is dropped (its run was cancelled) cancels and deletes its session without waiting, and the runtime's shutdown cancels and deletes again every session a call left open, waiting 5 s at most, before it ends Grok Build. A command that exits on an interrupt kills Grok Build at once (§1), so its sessions may stay.
- **Answer.** The first image tool call that completed: its `rawOutput` names the saved image (`type` `ImageGen` or `ImageEdit`, and `path`). FX reads it only when `path` is absolute, holds no `.` or `..`, names a file `<digits>.<jpg|jpeg|png|webp>` in a folder `images` of a folder named after the call's session, and is a regular file (not a link) of 1 byte to 64 MiB; else `Grok Build reported an image FX does not read`. FX then cancels the rest of the turn and reads the prompt's answer until it comes, or 3 s. Grok Build saves JPEG images: the answer is file `image`, the image's pixels encoded as PNG (a PNG is kept as it is), and `data: null`. Cost: `None`, since Grok Build reports none for its subscription: the route's hold, $0, is what is charged, and the record says the cost was not reported. Usage (§6): billing `plan`, `runtime` `grok <version>`, `agent_model` (the answer's `_meta.modelId`), and the turn's total `input_tokens`, `cached_input_tokens` and `output_tokens` (the answer's `_meta.usage`: `inputTokens`, `cachedReadTokens`, `outputTokens`), each when Grok Build gave it. Grok Build's own dollar figure for its agent's tokens is not booked or kept. A version or model is kept only as for Codex (§9.6).
- **Outcomes.** No error is known to prove that Grok Build did not receive a request, so none is `NotReceived`. A failure is `Failed { cost: None, retryable: false }`, and that call is never sent again by the engine: before `session/prompt` is written a failure is most likely to repeat; once it is written, an image may already have been made and counted against the user's subscription. The one exception: an image tool call that Grok Build reports as failed saved no image. When Grok Build's words for it name an HTTP status from 400 to 499 other than 408 and 429, xAI refused the request and would refuse it again: the reason is `Grok Build's image tool was refused (HTTP <status>)`, and the failure is final. Otherwise (and when they do not name SuperGrok) the reason is `Grok Build's image tool failed`, and that failure is `retryable: true`, so the engine may make a new attempt (§4.1), each a new, recorded attempt that may draw on the user's subscription. In real runs such failures came from a passing network error between Grok Build and xAI. The adapter itself never sends a call again. The reason starts `Grok Build image generation was not started: ` before `session/prompt` is written, and `Grok Build image generation failed: ` after, followed by:
  - `Grok Build exited`, `Grok Build's output is unreadable`, `Grok Build sent a line that is not JSON`, `Grok Build sent a message over 64 MiB`, `Grok Build could not read a request`, `Grok Build started no session`, `Grok Build has no pipes`, `the command is stopping`;
  - `Grok Build refused <method> (error <code>)` for an error response;
  - `Grok Build did not take the prompt within <n> s`, or `Grok Build did not finish within <n> s`;
  - `Grok Build called <tool>, a tool FX does not allow`;
  - for an image tool call that failed: `Grok Build's image tools need a SuperGrok subscription on the signed-in account` when Grok Build's words for it name SuperGrok (and later calls are refused, above), `Grok Build's image tool was refused (HTTP <status>)` for a refused request, else `Grok Build's image tool failed` (above); for an image FX does not read, the sentence above; for one it cannot decode, `Grok Build's image is not decodable`. A later good image of the same turn wins over these;
  - for a prompt answered without an image: `Grok Build answered without an image`, or, for a `stopReason` other than `end_turn`, `Grok Build's turn ended <reason> without an image`.

  `<tool>` and `<reason>` are Grok Build's words only when they are letters, digits and `_`, at most 40 of them, else `unknown`. Grok Build's own messages are never shown: they can name private paths (§8).
- **Check:** the image checks of [capabilities.md](capabilities.md) §2. An answer that fails them is a billed attempt like any other (§4.4).
- **Reasons:** the label is `Grok Build image generation`.

## 10. The built-in route table

FX ships a default route table, embedded in `grida-fx`. It is the first table of the catalog ([identity.md](identity.md) §7): a project's `route_tables` override its entries, and any `--routes` file leaves it out.

- It holds 29 routes over 10 capabilities. Every route MUST be served by an adapter of §9, and every capability is defined in [capabilities.md](capabilities.md).
- **Prices are planning allowances** in US dollars, not provider quotes. What a call costs is settled from what the provider reports, or else from the whole hold (§6). The video routes are priced per second, in tiers by `resolution`.
- **The Codex routes, `imagegen@codex` for `image.generate` and for `image.edit`, are free:** $0 to $0, untiered. They run on the user's own Codex plan, which FX neither sees nor bills (§9.6). Their adapter bills the user's plan (§6), so a live run whose calls are all on them needs no ceiling. They declare `image_input`, `flexible_size`, `alpha`, `auto_background`, `opaque_background` and `png_output`, and the `image.generate` route also `text_to_image`: the controls their adapter honours, so no exact size and no mask. Both take at most 5 input pictures.
- **The Grok Build routes, `imagine@grok` for `image.generate` and for `image.edit`, are free:** $0 to $0, untiered. They run on the user's own Grok subscription, which FX neither sees nor bills (§9.8). Their adapter bills the user's plan (§6), so a live run whose calls are all on them, or on them and the Codex routes, needs no ceiling. They declare `image_input`, `flexible_size`, `auto_background`, `opaque_background`, `jpeg_output` and `png_output` (Grok Build draws JPEG; the answer is the same pixels as a PNG), and the `image.generate` route also `text_to_image`: so no transparency, no exact size and no mask. Both take at most 3 input pictures.
- **The Grida Gateway routes (`@gg`) take the prices of the same model on another provider.** GG charges a model's regular tariff (§9.7), so `openai/gpt-image-2.5-sunburst@gg` has OpenAI's image tiers, the text routes OpenRouter's prices for the same models, `alibaba/wan-3.0@gg` $0.05 a second at `480p`, $0.10 to $0.20 at `720p` and $0.20 at `1080p`, up to 30 seconds, and the 3D routes Tripo's prices for the same models. A GG call is booked at GG's charge once it settles, which GG reports for every call (§9.7); one whose charge has not settled in time is booked at its whole hold, as any call without a reported cost is (§6).
- **The other image routes are priced per call, in tiers by `size`** ([identity.md](identity.md) §12). A tier is named by the exact size text, such as `1024x1024`. A request whose `size` no tier names (`auto`, none, or any other `<W>x<H>`) is priced at the route's whole range. The plan prices a step by its `size`, and each attempt's hold is the high price of the request's own tier ([protocol.md](protocol.md) §6.1 step 6).
  - A tier's low is the image output at quality max for that size, at $30 per million output tokens, rounded down to the cent. The output is measured or, where only a call's bill is known, derived from that bill less an estimate of its input. The estimate is the same for every such bill: the measured input of one comparable call, a prompt and one input picture ($0.006314). Each such bill is below every high of its tier. A tier's high adds an allowance for the input, rounded up to the cent: $0.07 for a generate's prompt, and $0.21 for an edit's 16 input pictures and long prompt.
  - The whole range contains every tier: $0.09 to $0.70 for a generate and to $0.85 for an edit, from $0.13 on OpenRouter.
  - **OpenAI's highs are what an OpenAI call is booked at.** OpenAI reports token usage but no cost (§9.1), so every OpenAI image call is booked at its whole hold, and a high below what a call can cost would under-count it. Each OpenAI high leaves room for one image output at its size, measured or derived as above. The fal and GG routes serve the same model in the same envelope, and take the same prices.
  - **OpenRouter's highs cover twice the output.** OpenRouter reports its cost (§9.2), so its high is only the hold. It has billed a 1024x1024 generate $0.42164: twice the output the same size cost at OpenAI directly (2 × $0.21072 = $0.42144), plus its prompt. Its tiers name only the sizes it serves (§9.2).
  - **An open risk.** That one bill does not show whether OpenRouter counts the output twice or the model now draws twice as much. If the model does, an OpenAI call, and a fal call whose answer reports no cost, can cost one more output than its high, and is under-counted by up to that much: a 1024x1024 generate would cost about $0.42 and be booked at $0.29. So the OpenAI and fal highs are not known to be worst cases. OpenAI's usage report for a call made since that bill settles it: one output means the doubling is OpenRouter's; two mean these highs need the same room as OpenRouter's.
  - A project that knows its calls better restates a route in its own route table, with tighter prices ([identity.md](identity.md) §7).
- **Contracts are identity.** Each contract object enters its route fingerprint, and so every cache key the route serves. Changing a contract changes every key under it.
  - A contract's `adapter` is the name §9 gives the adapter that serves the route, and its `adapter_behavior` is the behaviour §9 names, where §9 names one.
  - The image routes carry `adapter_behavior` as a string (`"1"`, `"3"`), and every other route carries it as the integer `1`. Each adapter serves its behaviour only as spelled here: the string and the number are different contracts, with different fingerprints.
  - The two `openai/gpt-6-astra@openrouter` routes carry the `request_policy` of §9.2, and the structured one also `pictures: unchanged`. The two `openai/gpt-6-astra@gg` routes carry the same choices in GG's members (§9.7): `image_detail: high` and `reasoning_effort: high`, and the structured one also `pictures: unchanged`.
  - The GG image routes carry `quality: max`, which the OpenAI adapter always sends (§9.1), the GG 3D routes the `endpoint` they submit to, and the GG rig route its `check_endpoint`.
- **Features** are the names [capabilities.md](capabilities.md) lists for each capability, and a route declares only what its adapter honours. So the OpenRouter image routes declare neither `alpha` nor `mask`, the GG image routes no `mask`, the GG video route no `first_last_frame` (it takes a first frame only), the GG mesh route no `quad`, and no `image.generate` route but Codex's and Grok Build's declares `image_input`, since every other one refuses references (§9).
- **Input-picture limits are declared.** A route that takes input pictures declares `max_input_pictures`, the most it takes in one call, as its adapter refuses more: 16 on the OpenAI, OpenRouter and fal `image.edit` routes, 5 on both Codex routes, and 3 on both Grok Build routes. The GG routes declare none: GG checks each model's own limits (§9.7). The plan refuses a step that would send more ([capabilities.md](capabilities.md) §1).
- **Pacing is not identity.** The engine applies a route's pacing per route id, so routes of one id for several capabilities share it. The OpenAI and OpenRouter image routes declare `requests_per_minute: 150`. The 3D routes and the video routes declare `concurrency: 1`, and so do the GG image routes: GG runs one GPT Image call per organization at a time until it settles, and answers a second with a 429 (§9.7). The Codex routes declare `concurrency: 3`, shared by their one id: one Codex runs several turns at once (five at once drew in about the time of one), and three keeps the user's plan from being spent faster than they would by hand; a project sets its own in `fx.yaml`. The Grok Build routes declare `concurrency: 3` likewise, on their one Grok Build. A call waiting for its slot is recorded as waiting ([store.md](store.md) §8). The structured routes declare `concurrency: 4`, and the `openai/gpt-6-astra` agent routes declare `concurrency: 1`.
- **Shipped without a route:** `background.remove` has an adapter on fal, but no route until fal's price for it is recorded. `vision.annotate`, `vision.review` and `structured.review` have no adapter. Calls on any of these are `no_route` unless a project's table adds a route.

## 11. Renamed features

A feature has one name. These names are not features, and a step that requires one is refused
while planning as no feature of its capability, naming FX's instead
([capabilities.md](capabilities.md) §1): `masked_edit` is `mask`, `transparent_background` is
`alpha`, and `reference_images` and `data_url_reference_input` are `image_input`.
