# Adapters

Version 1.

The adapter contract: where adapters run, the transport every adapter shares, how each provider answer becomes an attempt's outcome and its bill, refusals, costs, long jobs, downloads and reasons, provider options, and the binding of a project's own adapters, which run out of process (§9). What a capability's request and answer hold is [capabilities.md](capabilities.md). The providers FX speaks, their keys and endpoints, the adapters FX ships and the route table are [providers.md](providers.md); each adapter FX ships has a reference beside its code, listed in [providers.md](providers.md) §2, which is normative for that adapter. The engine's side (the cache, holds, job records, the retry owner) is [protocol.md](protocol.md) §6.1 and [store.md](store.md) §5.

Words: an **adapter** turns one capability call into provider requests. A **send** is one exchange of the call's request (or one submit of a long job). An **attempt** is one reserved, recorded and settled hold; a send that provably was not received is resent under the same attempt. Every rule below serves the ratified rule ([overview](../docs/wg/overview.md), "Retry and billing"): **the engine is the only retry owner, and a request is resent only when it provably was not received.**

## 1. Where adapters run

- The engine registers adapters by capability, provider and the contract `adapter` each serves ([providers.md](providers.md) §2), and one per capability and provider as its default. For each provider the planning project declares with a `family`, it registers that family's adapters under the provider's name ([providers.md](providers.md) §1, §2); for each it declares with an `adapter`, it registers that project adapter (§9) for every capability the adapter declares, each as its capability's default on the provider. A route `model@provider` is served by the adapter registered for its capability on its provider under the name its contract's `adapter` gives, and by the default when the contract names none (absent, `null` or not a string). A contract that names an adapter none is registered under is served by none: a call on it that must be sent is refused with `no_route` ([protocol.md](protocol.md) §6.1). The model is the route's, never chosen by the adapter.
- Adapters exist only in a **live** invocation (`grida-fx run --live`). Without `--live` nothing that can reach a provider is constructed: every call that is not answered from the call cache is refused (`not_live`), and a recorded call replays offline ([store.md](store.md) §4).
- A **stand-in run** ([protocol.md](protocol.md) §5.7) is never live, and uses only the adapters' checks: it holds each stand-in's answer to the check of the adapter that would serve the route (§3.4). Those adapters are built with no keys, for FX's own declarations only, over a transport that refuses every exchange (§2 item 8), and nothing but their checks is called. A route on a declared provider so gets its capability's every-route check, which is all a family's adapters check ([providers.md](providers.md) §2), and all the engine checks of a project adapter's answer (§9): no adapter host is started.
- An adapter is constructed whether or not its provider's key is present. A call without the key is refused before anything is sent (§4).
- **Runtimes.** Some adapters work through a *runtime*: a program the user installed and signed into, which the invocation starts on the first call that needs it, shares between that call and the rest, and shuts down with the run. The engine owns a runtime's life: the run's cleanup shuts it down and counts it in its verification ([control.md](control.md)), and a command that exits on an interrupt kills it. A runtime may keep tasks of its own (reading and writing the program's pipes); they end with it. [Codex](../crates/grida-fx-providers/src/codex/README.md) and [Grok Build](../crates/grida-fx-providers/src/grok/README.md) are two, and none of §2 applies to them: only a live invocation looks for their programs to start them, and only when the network is on (§2 item 8), so no other invocation and no test can start it. A call through a runtime is safe to drop as every adapter's is: what it asked the runtime for is stopped. A project adapter's host (§9) is owned the same way, but it is not a runtime: it is a node host, and it starts with the network off too.
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
- An answer is accepted only after these checks, in order. The first is the **adapter's check**: judgements on a well-formed answer, such as the file's kind and signature, exact dimensions, alpha, or the answer against the request's schema. A project adapter makes its judgements inside its send and collect instead, and the engine first holds its answer to its shape, as a stand-in's (§9). For an adapter FX ships, and for a project adapter, the engine then makes the **every-route check** [capabilities.md](capabilities.md) gives the capability (its *Every route's check*, or *Check*), redacting what it quotes with the adapter's redactor (§7). The engine then round-trips the answer's `data` through canonical JSON, and checks the capability's shape ([protocol.md](protocol.md) §6.1 step 7). A refusal from any of them fails the attempt **as billed**, at the reported cost or else the whole hold, and the engine may make a new attempt.
- The adapters FX ships judge, on an answer to a request they sent, at least what the every-route check judges, with sentences of their own where their references give them, so the every-route check after them changes no outcome.
- Structural failures belong in the send (`Failed`). Judgements on a well-formed answer belong in the check. Both bill and both may be retried; the split only decides where the sentence comes from.
- An answer MUST hold the files named in [capabilities.md](capabilities.md), with their kinds, and the `data` named there. It holds nothing else: no request id, no usage, no provider URL.
- A stand-in's answer passes the same checks, once and billed at nothing: its shape first, since no adapter made it, then one route's check (the adapter's where FX has an adapter for the route, else the every-route check), the round trip and the capability's ([protocol.md](protocol.md) §6.1, *Stand-in answers*). An adapter's check MUST therefore judge an answer from the call (its route, request, files and take) and the answer alone, never from anything its own send or collect kept.

## 4. Refusals before sending

A call MUST be refused before any byte leaves (`Refused`, $0, `capability_refused`) in this order. The first refusal wins.

For an adapter FX ships, the engine makes steps 1 and 2 before the adapter sees the call: step 1 from the contract the adapter serves ([providers.md](providers.md) §2) and then the adapter's own judgement of its contract's other members, then step 2. For a project adapter (§9) it makes steps 1 to 3: step 1 from the adapter's identity, step 2, and step 3 from its declaration. It makes them inside the attempt, where the adapter's own refusal would come: after the call's place under the route's pacing, its hold and, for a long job, its `submitting` record ([protocol.md](protocol.md) §6.1 steps 6 and 7; [store.md](store.md) §5). So the events, the $0 settlement and the job record are the same whoever refuses, and the retry owner redacts the sentence as it redacts every reason (§7). The adapter makes the steps left: 3 to 5, or 4 and 5 for a project adapter, which judges any other member of its contract there. The engine makes neither step when it collects a long job (§6): the call's key already fixes its route's contract and its request.

1. **The route's contract is not one this adapter serves.** The `adapter` and `adapter_behavior` in the route's contract MUST be the adapter's own ([providers.md](providers.md) §2) when present: `<route> is not a <capability> route this adapter serves`. `adapter_behavior` is compared as canonical JSON ([identity.md](identity.md) §2), so the string `"1"` and the number `1` differ. A member that is absent or `null` is not checked. A route read from a route table has them filled in where its contract left them out ([identity.md](identity.md) §7), so a route a project declares without a contract is served. The engine hands a call only to the adapter its contract's `adapter` names, or to the default (§1), so `adapter_behavior` is what keeps a table from pointing a model at a wire its adapter no longer sends. An adapter whose reference names other contract members then judges them, with the same sentence followed by `: <why>`.
2. **The request does not fit its capability** ([capabilities.md](capabilities.md) §1). A member the capability does not define, a required member that is absent or `null`, or a member of the wrong type. The refusals of the later steps assume a request that passed this one: a required member is present, and every member has its type.
3. **The provider's key is missing**: `<VARIABLE> is not set`. A provider declared without a credential needs none ([providers.md](providers.md) §1). For [Codex](../crates/grida-fx-providers/src/codex/README.md) and [Grok Build](../crates/grida-fx-providers/src/grok/README.md), which have no key, their program is missing instead.
4. **A value the route cannot take**: provider options the adapter refuses (§8), first; then blank text, a number out of range, an unknown enum value, a size the route does not serve, too many pictures, or a mask on a route without masks. Each adapter's sentences are in its reference ([providers.md](providers.md) §2). An adapter whose reference says so also holds the request to what the route's table declares: its input pictures (`max_input_pictures`), the most units of a price per second (`max_units`), the tiers of a tiered price, and its features, such as `image_input` or `text_to_video`. A request beyond them is a value the route cannot take, judged here only: they are not part of the route's identity ([identity.md](identity.md) §7), so a table may change them under a job already sent (§6).
5. **A file**: a file value whose digest the call does not carry, a store copy that cannot be read or is empty (`<member> has no bytes to send`), or a file of a kind the route does not take.

Refusals are $0 because nothing left. A provider that would have answered the same request with a 4xx is not asked.

## 5. Costs

- **Billing.** An adapter says how its calls are billed: in **dollars**, to the account of the key FX sends, or against the user's own **plan** in a runtime they signed into ([Codex](../crates/grida-fx-providers/src/codex/README.md) and [Grok Build](../crates/grida-fx-providers/src/grok/README.md)), which FX neither sees nor books. Only an adapter FX ships makes a provider bill a plan, never a route table, a declaration or a project adapter (§9), which bills dollars. A live run whose calls are all on providers that bill a plan needs no ceiling: it runs under a ceiling of $0, and refuses a call on any other provider ([protocol.md](protocol.md) §6.1). An answer may also report what the call consumed besides dollars (tokens, the runtime and its version); the call record keeps it as `usage` ([store.md](store.md) §3).
- **A reported cost is the provider's own figure, in US dollars.** It must be a finite, non-negative JSON number. Booleans, strings and `null` are not costs. It is converted exactly from its JCS text ([identity.md](identity.md) §2) and MUST be **rounded up** to whole micro-dollars, so a cost is never under-counted: `0.00012345` is `$0.000124`, and `1e-7` is `$0.000001`. (This is the conversion of a reported figure. The half-even rounding of [identity.md](identity.md) §12 is for the engine's own arithmetic on prices.)
- **Credits.** A provider that reports credits converts them at its documented rate: [Tripo](../crates/grida-fx-providers/src/tripo/README.md), 1 credit = USD 0.01. The result is rounded up the same way.
- **Nothing is computed from tokens or price lists.** When the provider reports no cost, the cost is `None` and the engine charges the whole hold.
- **A capped route** has no known price, only a cap ([identity.md](identity.md) §12): each attempt holds the cap and is charged what the provider reports, else the whole cap. The cap is what FX holds, not a limit the provider honours: a reported cost above it is charged in full, as on any route, and can take the run past its ceiling by the difference.
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
- **The check of a collected answer** runs as for a plain answer. A refusal settles the record and fails the call, since drawing again is a new paid job. Neither `collect` nor this check reads a limit the route's table declares (§4 step 4): those are not in the call's key, so a job sent before its route's table changed is still collected, and checked against what its request asked for.

## 7. Downloads and reasons

**Downloads.**
- A result file at a URL MUST be fetched **once** per send or collect, at the URL exactly as the provider gave it, on the download lane: no credential, no redirects, the response cap the adapter's reference gives, and the provider's host allowlist where the reference gives one: its declared download hosts ([providers.md](providers.md) §1). A failed fetch is not repeated within that send or collect. An adapter does not re-serialize the URL; it parses it only to check it. The default transport sends the URL as the WHATWG URL parser (Rust `url` crate, via reqwest, which has no raw-URI path) serializes it: in an http(s) query, controls, space, `"`, `<`, `>`, `'` and non-ASCII are percent-encoded; existing `%XX` escapes are kept; a default port is dropped, the host is lowercased, and `.`/`..` path segments are resolved.
- A download URL may be signed. It MUST NOT be kept in an answer's `data`, a handle, a record, an event or a reason. Adapters add it to the redactor before using it.
- A data URL in an answer is decoded in place; nothing is downloaded.

**Reasons.**
- Every sentence an adapter reports MUST be built from fixed text and allowlisted fields. Examples: `<label> returned HTTP <n>`, `: <safe detail>` (at most 720 characters), a request id that matches `^[A-Za-z0-9_.:-]{1,96}$`, and the bounded `type`/`code`/`param` of an OpenAI-style error envelope.
- It MUST NOT hold a key, an authorization header, a signed URL, an upload token, a data URL, a prompt or a response body.
- Every reason passes through redaction last. Each configured key and each added secret becomes `[redacted]`; known secret shapes are masked (data-URL payloads, base64 members such as `b64_json`, base64 runs of 80 or more characters, `sk-…` keys, `authorization` values, `api_key`/`token`/`secret`/`credential` members, URL queries). Whitespace is collapsed, and the result is cut to 500 characters ending in `…`. Text is redacted before it is cut, so a cut never leaves part of a secret.
- The engine's retry owner redacts once more every reason it reports (`capability_refused`, `call_failed`, `job_unsettled`), with a redactor over every key of the invocation: the adapter's own sentence, its check's, the engine's refusals before a send (§4), and the engine's checks of the answer, which may quote what the provider sent back (a `content-type`, a value the schema refused).
- `<label>` is the provider and the call, such as `OpenAI image generation` or `fal video submission`. Each adapter's labels are listed in its reference.

## 8. Provider options

A request's `provider_options` ([capabilities.md](capabilities.md) §1) and a route's `contract.provider_options` pass parameters FX does not expose to the provider. Each is an object by provider, and each member an object of that provider's own parameters: `{"fal": {"seed": 7}}`. The route's is part of its contract, so of its fingerprint ([identity.md](identity.md) §7); the request's is part of the call key ([identity.md](identity.md) §9).

- **Only the route's provider's member is read.** An adapter reads `provider_options.<provider>` for its route's provider, the route's and then the request's, and no other member: a step may give options for each provider it may run on.
- **The merge is shallow.** The route's parameters come first, in their order; a request's parameter replaces the route's of the same name in place, and its others follow. A parameter is replaced whole, never merged into. The parameters are added after FX's own members of what the adapter sends: the JSON body, or the JSON its multipart body carries, or text fields of their own (a string as itself, any other value as its JSON text). Each adapter's reference says where. Absent, `null` and `{}` add nothing, so a call without options sends what it sent before, byte for byte.
- **Refusals**, the first of step 4 (§4), before any byte leaves, a long job's free phase included; the route's options before the request's:
  - a member that is not an object: `<where> is an object`, `<where>` being `contract.provider_options` or `contract.provider_options.<provider>` for the route's, and `provider_options.<provider>` for the request's (the request check refuses a `provider_options` that is not an object, step 2);
  - a parameter FX owns: `<where>.<name> is a member FX owns`. An adapter owns the members it sets itself, whether or not this call sends them; those it never sends by design (OpenAI's `seed`, a mask GG refuses); and those that would change what it reads in the answer or restate a member it sets (`stream`; `max_tokens` beside `max_completion_tokens`). Each adapter's reference lists them;
  - a file value anywhere in the member ([protocol.md](protocol.md) §3.2): `<where>.<name> holds a file: provider options are JSON only`, `<name>` being the parameter that holds it, or `<where> holds a file: provider options are JSON only` when the member is one.
- **The runtimes take none.** [Codex](../crates/grida-fx-providers/src/codex/README.md) and [Grok Build](../crates/grida-fx-providers/src/grok/README.md) send a runtime no parameters, so a member for their provider, the route's or the request's, that is not absent, `null` or `{}` is refused, as their references say. Members for other providers are not read.
- **Never a credential.** A request is recorded as it is ([identity.md](identity.md) §9), and a route's contract is in a route table the project keeps with its files, so neither holds a key: a key comes only from its provider's variable ([providers.md](providers.md) §1). FX cannot tell a credential from a parameter, so it does not look for one.
- **The rest is the provider's.** A parameter the provider does not take is its to refuse (§3.3). One that changes its answer so that a check refuses it fails the attempt as billed (§3.4).

## 9. Project adapters

A project's own adapter is this contract's second binding: Python or TypeScript code in the planning project, which a node host runs out of process and the engine reaches over the node protocol ([protocol.md](protocol.md) §5.8, §6.8). Everything above holds for it, with what this section adds. The SDKs' authoring API is [protocol.md](protocol.md) §8.

**Declaring.**
- A declaration's `adapter` ([providers.md](providers.md) §1) names it as `<path>#<attribute>`: a module of the planning project, its path POSIX and relative to the planning project's root, without `./` and without `..` segments, and the module-level name it is declared under (`adapters/acme.py#acme`; in TypeScript, an export of the module). The module's suffix chooses its host as a node module's does ([protocol.md](protocol.md) §1), and the module must lie inside the planning project once symbolic links are resolved. A module in a package listed under `sources:` is reached through a project module that imports it, whose closure then holds the package's files.
- The adapter declares the capabilities it serves, each as a **request** (sent once per send, §3) or a **job** (submitted once and collected, §6), and optionally a version, an integer of at least 0. It serves each of them on its provider, as that capability's default there (§1). A capability [capabilities.md](capabilities.md) does not define (§13 there, or a project's own) may be served too; the engine then checks only what it checks of a stand-in's answer for such a capability.

**Loading, identity and the lock.**
- While planning, before it reads the route tables, the engine loads each adapter the planning project declares with `adapter.load` on the planning host of the adapter's language ([protocol.md](protocol.md) §5.8). It learns the adapter's version, its capabilities and their kinds, and its source closure, which it reads and hashes itself, as a node type's ([identity.md](identity.md) §6). Planning a project that declares an adapter therefore needs its language's node host, which runs the module's top-level code as describing a node module does. A plan, a dry run, a stand-in run and an offline replay need nothing more: no adapter host is started.
- An adapter that cannot be loaded refuses the command before anything is planned (exit 2): `fx.yaml: providers.<name>.adapter: <sentence>`. The sentence is the host's (`<path> failed to import: …`, or `<path>: <attribute> is not declared with adapter()`, in TypeScript `<path>: <attribute> is not an export declared with adapter()`); `no file <path>`; `<path> is outside the project`; a suffix sentence of [protocol.md](protocol.md) §1 (`no node host for .<suffix> modules`); the host's own unavailability; `the node host answered adapter.load with something FX cannot read: <where>`; `<path>#<attribute> declares no capability`; or, from a host whose SDK predates adapters (`-32601`), `the project's <package> <version> cannot load adapters: upgrade <package>`, the package being `grida` or `@grida/fx`.
- **Identity.** The adapter's contract name, `adapter`, is `<path>#<attribute>`. Its behaviour, `adapter_behavior`, is its version, an integer, when it has one; otherwise the string `source:` followed by the digest of its source: [identity.md](identity.md) §6's `fx-node-source-v1` object, its `files` the adapter's source closure and its `resources` empty. A route's contract is completed with both ([identity.md](identity.md) §7), and §4 step 1 holds a call's contract to them. So moving or renaming the module or the attribute, or changing the version, changes the fingerprint of every route the adapter serves, and an unversioned adapter's routes change with any edit to a file of its closure.
- **The lock.** A versioned adapter's source digest is recorded in `fx.lock` under `adapters`, keyed `<path>#<attribute>@<version>`, by `grida-fx lock`, which loads every adapter the project declares. A step whose route is served by a versioned adapter whose source no longer matches its entry is refused while planning: `<path>#<attribute>: its source changed but version <v> did not; bump the version, or confirm no change in behaviour with grida-fx lock --same <path>#<attribute>`. A versioned adapter with no entry plans normally, and `grida-fx lock --check` reports it as unlocked, as it does a node type ([identity.md](identity.md) §6).

**The adapter host.**
- Only a live invocation sends a call to a project adapter. It starts the adapter's **adapter host** on the first call that needs it: a node host of the adapter's language, found with the planning project as its project and started in the planning project's root ([protocol.md](protocol.md) §1), outside the pools of node hosts, one for each declaration with an adapter. The host loads the adapter with `adapter.load` and then serves its calls, several at once. The engine owns its life as a runtime's (§1): the run's cleanup shuts it down and counts it in its verification, and a command that exits on an interrupt kills it. Unlike a runtime's program, the host starts with the network off too (§2 item 8): only what the adapter sends through `fetch` is refused then.
- **Its environment** is FX's, without the variables of every other declaration, built-in or declared, and with its own declaration's variables, its key's and its base URL's, as FX read them, from the environment or the planning project's `.env` ([providers.md](providers.md) §1). Its code so reads its own key, and no other.
- **The plan stands.** The host's `adapter.load` must answer what planning learned: the same version, capabilities and kinds, and a source closure with the same digest. Otherwise, and when the host cannot be started or the adapter loaded, the host is not started again in that run, and each send and submit on the adapter is refused before anything is sent (`Refused`, $0), each collect `Unreachable` (its job may still run): `the adapter <path>#<attribute> changed since the run was planned`, or `the adapter host for <path>#<attribute> could not be started: <message>`.
- **A lost host.** When the host exits, or its connection ends, each call pending on it ends as its unknown outcome demands, and the next call starts a new host: a pending send is `Failed { cost: None, retryable: false }`, since the provider may have received it and a new attempt would hold again (Codex's precedent); a pending submit is `Uncertain`; a pending collect is `Unreachable`. The reason is `the adapter host exited with status <n>` (`… was killed by signal <n> (<NAME>)`, `the adapter host exited`). An error answer other than `cancelled`, or a result FX cannot read, ends its call the same way: `the adapter <path>#<attribute> failed: <message>`, or `the adapter <path>#<attribute> answered something FX cannot read: <where>`. A send or a submit whose request was never written whole to the host, its connection having ended first, provably did not reach the provider: it is `NotReceived`, with the same reason, and the engine sends it again (§3.3), to a new host; such a collect is `Unreachable`.
- **Concurrency and cancellation.** Each call is sent to the host when its attempt comes, under the route's concurrency and pacing as any call's: several may be pending at once, answered in any order. A call the engine drops (§1: it MUST be safe to drop) is cancelled with `$/cancel`; the adapter SHOULD stop its work, and the engine discards whatever the host answers later. The engine sets no deadline on an adapter's request: as an adapter FX ships sets the deadline of each exchange, the adapter sets its own (`fetch`'s `timeout_s`), and a long job's collect polls with its own clock until its own collect deadline (§6).

**Outcomes.** The host answers each request with one of the outcomes of §3 and §6, named by its `outcome` ([protocol.md](protocol.md) §5.8):

| Method | `outcome` | The outcome |
|---|---|---|
| `adapter.send` | `answered` | `Answered(answer)` |
| | `not_received` | `NotReceived { retry_after }` |
| | `refused` | `Refused` |
| | `failed` | `Failed { cost, retryable }` |
| `adapter.submit` | `accepted` | `Accepted { handle }` |
| | `uncertain` | `Uncertain` |
| | `not_received`, `refused`, `failed` | as for a send |
| `adapter.collect` | `answered` | `Answered(answer)` |
| | `ended` | `Ended { cost }` |
| | `unreachable` | `Unreachable` |

- **Costs.** `cost_usd` is the provider's own figure in US dollars, read and rounded up as §5 says. Left out, the provider reported none, and the engine charges the whole hold, or a capped route's cap. A project adapter bills dollars, to the account of its declaration's key: no declaration can claim a plan. An answer's `usage` may report tokens, which the call record keeps under `billing: dollars` ([store.md](store.md) §3).
- **Waits.** `retry_after_s` is the wait the provider asked for, in seconds, which the engine bounds and honours as §3.3 says.
- **Handles.** A handle is a JSON object holding only what collecting needs (§6). A submit accepted with a handle that is not an object, or whose text holds a key's value, is `Uncertain`: the provider may have taken the job, and FX cannot keep that handle.
- **Answers.** An answer's files are bytes with their kinds, as a stand-in's are ([protocol.md](protocol.md) §5.7), and it holds what [capabilities.md](capabilities.md) names and nothing else. Where that section says the engine takes `data` from the answer's file (`video.generate`, `mesh.generate`), the adapter answers `null` and the engine writes it. The adapter's own judgements of an answer (§3.4) are made in its send and collect, which answer `failed`, or `ended` for a collected job, with its own sentence: no `adapter.check` exists, so a stand-in's answer on its route gets the every-route check (§1).

**What the engine still checks.** The engine stays the only retry owner: it reserves, records and settles each attempt, resends only a `not_received`, makes at most 6 sends in all, and writes every job record ([store.md](store.md) §5). Around the adapter it makes, in order:
1. §4 steps 1 to 3 before the host sees a call: the route's contract against the adapter's identity, the request against its capability, and the declaration's key, `<VARIABLE> is not set`;
2. the handle of an accepted submit, as above;
3. an answer's shape, as a stand-in's ([protocol.md](protocol.md) §6.1, *Stand-in answers*, step 3.1), since FX did not shape it; then the every-route check, the round trip of its data and the capability's check (§3.4). Each refusal fails the attempt as billed, at the reported cost or else the whole hold, and a refusal of a collected answer settles the job;
4. every reason, redacted with the invocation's redactor, which knows every declared key's value, and cut to 500 characters (§7), before it is recorded or reported. The engine adds the URL of every download the adapter makes through `fetch` to the redactor of the call's reasons. It does not redact what the host prints on its standard error ([protocol.md](protocol.md) §1).

**Reasons** follow §7: built from fixed text and fields that cannot hold a secret, never a key, an authorization header, a signed URL, a prompt or a response body.

**The network.** A project adapter reaches the network however it likes: its own HTTP client, or a vendor's SDK. It MUST keep §2 items 1 to 3 itself: one exchange per send with its client's retries off, no redirect followed with a credential, and a deadline. It MUST keep item 8 too, which the engine cannot keep for a client it does not own: with `GRIDA_FX_NETWORK=off`, which its host's environment holds as the engine's does, it sends nothing through a client of its own, and answers as for a request refused before it left. It MUST give an honest outcome: `not_received` only when the provider provably did not receive the request, and `failed` or `uncertain` whenever it may have. `fetch` ([protocol.md](protocol.md) §6.8) makes one exchange through the engine's transport (§2) instead, which adds:
- the credential, attached by the engine from its own copy on the provider and upload lanes when the adapter asks for it, and only to the origin (scheme, host and port) of its declaration's base URL, so the adapter's code need not handle the key;
- the declaration's download hosts ([providers.md](providers.md) §1): a download outside them is refused, so a declaration that lists none downloads nothing through `fetch`;
- the network switch (§2 item 8), the response cap and the deadline, at most 64 MiB and 300 seconds;
- the failure's phase, which the adapter maps onto its outcome as §3.2 says: `refused` (the engine or the transport refused the request before it left) is `refused`; `not_sent` is `not_received`; `after_send` is `failed`, retryable, or `uncertain` for a submit's paid request.

`fetch` takes GET and POST only, and plays no recorded exchange: offline replay through it is not available yet.
