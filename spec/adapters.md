# Adapters

Version 1.

The adapter contract: where adapters run, the transport every adapter shares, how each provider answer becomes an attempt's outcome and its bill, refusals, costs, long jobs, downloads and reasons. What a capability's request and answer hold is [capabilities.md](capabilities.md). The providers FX speaks, their keys and endpoints, the adapters FX ships and the route table are [providers.md](providers.md); each adapter FX ships has a reference beside its code, listed in [providers.md](providers.md) §2, which is normative for that adapter. The engine's side (the cache, holds, job records, the retry owner) is [protocol.md](protocol.md) §6.1 and [store.md](store.md) §5.

Words: an **adapter** turns one capability call into provider requests. A **send** is one exchange of the call's request (or one submit of a long job). An **attempt** is one reserved, recorded and settled hold; a send that provably was not received is resent under the same attempt. Every rule below serves the ratified rule ([overview](../docs/wg/overview.md), "Retry and billing"): **the engine is the only retry owner, and a request is resent only when it provably was not received.**

## 1. Where adapters run

- The engine registers one adapter per capability and provider. A route `model@provider` is served by the adapter registered for its capability on its provider; the model is the route's, never chosen by the adapter.
- Adapters exist only in a **live** invocation (`grida-fx run --live`). Without `--live` nothing that can reach a provider is constructed: every call that is not answered from the call cache is refused (`not_live`), and a recorded call replays offline ([store.md](store.md) §4).
- A **stand-in run** ([protocol.md](protocol.md) §5.7) is never live, and uses only the adapters' checks: it holds each stand-in's answer to the check of the adapter registered for the route's capability and provider (§3.4). Those adapters are built with no keys over a transport that refuses every exchange (§2 item 8), and nothing but their checks is called.
- An adapter is constructed whether or not its provider's key is present. A call without the key is refused before anything is sent (§4).
- **Runtimes.** Some adapters work through a *runtime*: a program the user installed and signed into, which the invocation starts on the first call that needs it, shares between that call and the rest, and shuts down with the run. The engine owns a runtime's life: the run's cleanup shuts it down and counts it in its verification ([control.md](control.md)), and a command that exits on an interrupt kills it. A runtime may keep tasks of its own (reading and writing the program's pipes); they end with it. [Codex](../crates/grida-fx-providers/src/codex/README.md) and [Grok Build](../crates/grida-fx-providers/src/grok/README.md) are two, and none of §2 applies to them: only a live invocation looks for their programs to start them, and only when the network is on (§2 item 8), so no other invocation and no test can start it. A call through a runtime is safe to drop as every adapter's is: what it asked the runtime for is stopped.
- An adapter MUST NOT retry, pace, sleep (except while polling a long job, §6, or waiting for the charge of a request it made, as [GG's adapters](../crates/grida-fx-providers/src/gg/README.md) do) or write to the store: it returns bytes and data, and the engine stores them. It MUST be safe to drop at any point: it starts no detached task and leaves nothing behind.

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
9. **Proxies never see a credential.** A request whose URL is plain `http`, or whose host is a loopback host (`localhost`, a loopback IP), goes straight to its host, whatever the environment's proxy settings say: a proxy would read a plain request, credential included, in clear text, and a loopback host is this machine, not the proxy's. Any other request is `https` to a remote host, and honours the proxy settings of the environment (`HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`; `HTTP_PROXY` names a proxy for plain `http` only, so it never applies). Through a proxy it is tunnelled (`CONNECT`): the proxy sees the host and port, and the request stays encrypted to the provider, unless the platform trusts the proxy's own certificates (item 10). These variables are the transport's; they are not keys or endpoints ([providers.md](providers.md) §1).
10. **Certificates chain to a trusted root.** A server's certificate is accepted when it chains to a well-known public root or to a root the platform's certificate store trusts, as the machine's other programs accept it. A proxy that presents certificates under a root the platform trusts can read a tunnelled request, credential included; which roots a machine trusts is its owner's choice, not the transport's.

The default transport is HTTP over rustls, trusting the bundled Mozilla roots and the platform's certificate store (`SSL_CERT_FILE` / `SSL_CERT_DIR` in its place when set). Besides what the request sets, it sends `accept-encoding: identity`, `accept: */*` unless the request sets `accept`, and a `user-agent` on the provider and upload lanes. Tests use a replay transport that plays synthetic exchanges and asserts every request, or one that fails any send. The default transport MUST NOT be constructed in a test. A request, a response or a credential printed for debugging shows no URL path or query, no header value, no body and no key.

## 3. Outcomes

### 3.1 The outcomes of one send

| Outcome | Meaning | Billing | What the engine does next |
|---|---|---|---|
| `Refused` | refused before anything was sent (§4), or the transport refused the request itself | $0 | `capability_refused`, never retried |
| `NotReceived { retry_after }` | provably not received: the exchange failed `not_sent`, or the provider said it took nothing (§3.3) | $0 | resends under the same attempt (at most 6 sends in all), after its backoff or the provider's `retry_after` when that is longer, and never more than 60 s later (§3.3) |
| `Failed { cost, retryable }` | the provider received the request and the call failed | the reported cost, else the whole hold | a new attempt when `retryable` and sends are left; else `call_failed` |
| `Answered(answer)` | the provider answered | the answer's reported cost, else the whole hold | the adapter's check, then the engine's checks (§3.4) |

A long job's outcomes are in §6.

### 3.2 Transport failures

| Failure | Plain request | Long-job submit (the paid request) |
|---|---|---|
| `not_sent`, refused by the transport | `Refused` | `Refused` |
| `not_sent`, otherwise | `NotReceived` | `NotReceived` |
| `after_send` (a timeout, a reset, a truncated body, a body over the cap) | `Failed { cost: None, retryable: true }` | `Uncertain` |

### 3.3 HTTP statuses

The default classes are below. Each adapter's reference ([providers.md](providers.md) §2) may override a row, and says where it does.

| Status | Plain request | Long-job submit (the paid request) |
|---|---|---|
| 2xx | parsed (§3.4) | parsed into a handle; a 2xx without a usable handle is `Uncertain` |
| 3xx | `Failed { cost: None, retryable: false }`: `<label> was redirected (HTTP <n>); check <BASE_URL variable>` | `Uncertain` |
| 408 | `NotReceived`: the server says it did not receive the whole request | `NotReceived` |
| 429 | `NotReceived`: the provider took nothing. Its `retry-after` header sets `retry_after` | `NotReceived` |
| any other 4xx | `Failed { cost, retryable: false }`: a deterministic refusal, so resending cannot help. `cost` is `Some(0)` only where the adapter's reference says the provider documents such refusals as unbilled; else `None` | `Failed { cost, retryable: false }`, the same way |
| 5xx | `Failed { cost: None, retryable: true }` | `Uncertain` |

`Some(0)` is never inferred. A provider's refusal is $0 only where its adapter's reference says so, and the reference records why.

**The wait a provider asks for.**
- A `NotReceived` (or a submit's `NotReceived`) built from an HTTP response carries `retry_after` when the response asks for a wait: `retry-after-ms` in milliseconds, else `retry-after` in seconds (fractions allowed) or as an HTTP date. A header that cannot be read is ignored. A `NotReceived` from a transport failure carries none.
- The engine waits the longer of its backoff and `retry_after` before the next send, and never more than 60 s. The wait is pacing, not billing: it changes neither the outcome nor the attempt count.
- The reason may name the wait; it never quotes the response's other headers.

### 3.4 Answers and checks

- A 2xx whose body cannot become an answer (not JSON, a missing member, invalid base64, a URL where bytes were expected, a download that fails) is `Failed { cost, retryable: true }`. `cost` is the reported cost when the body was read far enough to find it, else `None`. The provider did the work.
- An answer is accepted only after three checks. The first is the **adapter's check**: judgements on a well-formed answer, such as the file's kind and signature, exact dimensions, alpha, or the answer against the request's schema. The engine then round-trips the answer's `data` through canonical JSON, and checks the capability's shape ([protocol.md](protocol.md) §6.1 step 7). A refusal from any of the three fails the attempt **as billed**, at the reported cost or else the whole hold, and the engine may make a new attempt.
- Structural failures belong in the send (`Failed`). Judgements on a well-formed answer belong in the check. Both bill and both may be retried; the split only decides where the sentence comes from.
- An answer MUST hold the files named in [capabilities.md](capabilities.md), with their kinds, and the `data` named there. It holds nothing else: no request id, no usage, no provider URL.
- A stand-in's answer passes the same checks, once and billed at nothing: its shape first, since no adapter made it, then the adapter's check, the round trip and the capability's ([protocol.md](protocol.md) §6.1, *Stand-in answers*). An adapter's check MUST therefore judge an answer from the call (its route, request, files and take) and the answer alone, never from anything its own send or collect kept.

## 4. Refusals before sending

An adapter MUST refuse a call before any byte leaves (`Refused`, $0, `capability_refused`) in this order. The first refusal wins.

1. **The route's contract is not one this adapter serves.** The `adapter` (and `adapter_behavior`, where the adapter checks one: [providers.md](providers.md) §2) in the route's contract MUST be the adapter's own when present: `<route> is not a <capability> route this adapter serves`. A member that is absent or `null` is not checked, so a route a project declares without a contract is served. The registry is keyed by capability and provider, not model, so this is the only guard against a table pointing a model at the wrong wire.
2. **The request does not fit its capability** ([capabilities.md](capabilities.md) §1). A member the capability does not define, a required member that is absent or `null`, or a member of the wrong type. The refusals of the later steps assume a request that passed this one: a required member is present, and every member has its type.
3. **The provider's key is missing**: `<VARIABLE> is not set`. For [Codex](../crates/grida-fx-providers/src/codex/README.md) and [Grok Build](../crates/grida-fx-providers/src/grok/README.md), which have no key, their program is missing instead.
4. **A value the route cannot take**: blank text, a number out of range, an unknown enum value, a size the route does not serve, too many pictures, or a mask on a route without masks. Each adapter's sentences are in its reference ([providers.md](providers.md) §2).
5. **A file**: a file value whose digest the call does not carry, a store copy that cannot be read or is empty (`<member> has no bytes to send`), or a file of a kind the route does not take.

Refusals are $0 because nothing left. A provider that would have answered the same request with a 4xx is not asked.

## 5. Costs

- **Billing.** An adapter says how its calls are billed: in **dollars**, to the account of the key FX sends, or against the user's own **plan** in a runtime they signed into ([Codex](../crates/grida-fx-providers/src/codex/README.md) and [Grok Build](../crates/grida-fx-providers/src/grok/README.md)), which FX neither sees nor books. Only an adapter makes a provider bill a plan, never a route table. A live run whose calls are all on providers that bill a plan needs no ceiling: it runs under a ceiling of $0, and refuses a call on any other provider ([protocol.md](protocol.md) §6.1). An answer may also report what the call consumed besides dollars (tokens, the runtime and its version); the call record keeps it as `usage` ([store.md](store.md) §3).
- **A reported cost is the provider's own figure, in US dollars.** It must be a finite, non-negative JSON number. Booleans, strings and `null` are not costs. It is converted exactly from its JCS text ([identity.md](identity.md) §2) and MUST be **rounded up** to whole micro-dollars, so a cost is never under-counted: `0.00012345` is `$0.000124`, and `1e-7` is `$0.000001`. (This is the conversion of a reported figure. The half-even rounding of [identity.md](identity.md) §12 is for the engine's own arithmetic on prices.)
- **Credits.** A provider that reports credits converts them at its documented rate: [Tripo](../crates/grida-fx-providers/src/tripo/README.md), 1 credit = USD 0.01. The result is rounded up the same way.
- **Nothing is computed from tokens or price lists.** When the provider reports no cost, the cost is `None` and the engine charges the whole hold.
- **Collecting** a long job a previous run submitted bills nothing new ([protocol.md](protocol.md) §6.1 step 5). A job that ends without a result in the run that submitted it bills what its adapter reports for the ended job (`Ended { cost }`, §6), else its whole hold. As for a refusal (§3.3), `Some(0)` is never inferred: an ended job is $0 only where its adapter's reference says so, and the reference records why.
- **A job the submitting run stopped waiting for keeps its whole hold.** That run settles the hold in full when the job is `Unreachable` (§6), since the job may still be running. A later run that finds the job ended, at whatever cost, bills nothing new and changes nothing already booked.

## 6. Long jobs

A long job (a video, a mesh, a rig) is submitted once and then collected by its handle ([store.md](store.md) §5).

**Submit.**
- A submit may begin with a **free phase**: requests the provider does not bill, such as file uploads or a free pre-check. A failure there bills nothing:
  - `Refused` when it is deterministic (a 4xx other than 408 and 429, or a pre-check that refuses the input);
  - `NotReceived` otherwise. The engine resends the whole submit, free phase included.
- Then the **paid request** is sent exactly once. Its outcome follows §3.2 and §3.3:
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
- **`Ended { cost }`** carries what the ended job costs: the provider's own figure where the adapter's reference reads one for a job that ended that way; `Some(0)` only where the reference says the provider does not bill such a job and it reports no figure; else `None`. The run that submitted the job settles its hold at that cost, else in full (§5). A result that is unusable came from a job the provider finished and billed, so it costs what the provider reported for that job, as an answer would, and `None` when the provider reported nothing that can be read.
- **The check of a collected answer** runs as for a plain answer. A refusal settles the record and fails the call, since drawing again is a new paid job.

## 7. Downloads and reasons

**Downloads.**
- A result file at a URL MUST be fetched **once** per send or collect, at the URL exactly as the provider gave it, on the download lane: no credential, no redirects, the response cap the adapter's reference gives, and the provider's host allowlist where the reference gives one. A failed fetch is not repeated within that send or collect. An adapter does not re-serialize the URL; it parses it only to check it. The default transport sends the URL as the WHATWG URL parser (Rust `url` crate, via reqwest, which has no raw-URI path) serializes it: in an http(s) query, controls, space, `"`, `<`, `>`, `'` and non-ASCII are percent-encoded; existing `%XX` escapes are kept; a default port is dropped, the host is lowercased, and `.`/`..` path segments are resolved.
- A download URL may be signed. It MUST NOT be kept in an answer's `data`, a handle, a record, an event or a reason. Adapters add it to the redactor before using it.
- A data URL in an answer is decoded in place; nothing is downloaded.

**Reasons.**
- Every sentence an adapter reports MUST be built from fixed text and allowlisted fields. Examples: `<label> returned HTTP <n>`, `: <safe detail>` (at most 720 characters), a request id that matches `^[A-Za-z0-9_.:-]{1,96}$`, and the bounded `type`/`code`/`param` of an OpenAI-style error envelope.
- It MUST NOT hold a key, an authorization header, a signed URL, an upload token, a data URL, a prompt or a response body.
- Every reason passes through redaction last. Each configured key and each added secret becomes `[redacted]`; known secret shapes are masked (data-URL payloads, base64 members such as `b64_json`, base64 runs of 80 or more characters, `sk-…` keys, `authorization` values, `api_key`/`token`/`secret`/`credential` members, URL queries). Whitespace is collapsed, and the result is cut to 500 characters ending in `…`. Text is redacted before it is cut, so a cut never leaves part of a secret.
- The engine's retry owner redacts once more every reason it reports (`capability_refused`, `call_failed`, `job_unsettled`), with a redactor over every key of the invocation: the adapter's own sentence, its check's, and the engine's checks of the answer, which may quote what the provider sent back (a `content-type`, a value the schema refused).
- `<label>` is the provider and the call, such as `OpenAI image generation` or `fal video submission`. Each adapter's labels are listed in its reference.
