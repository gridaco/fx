// Type-level tests of project adapters: `bun run typecheck` (tsc --noEmit) compiles this file and
// never runs it. Each `@ts-expect-error` marks code that must not compile; one that compiles anyway
// fails the check (TS2578). The rest must compile, with the types the `Equal` checks name.

import {
  type Accepted,
  type Adapter,
  type AdapterCall,
  type AdapterFile,
  type AdapterValue,
  Answered,
  adapter,
  type CollectCall,
  type CollectOutcome,
  FetchError,
  type FetchResponse,
  type JobHandle,
  type SendOutcome,
  type SubmitOutcome,
} from "../../src/index.js";
import type { JsonValue } from "../../src/workflow.js";

type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
function expectType<T>(_value: T): void {}
declare const bytes: Uint8Array;

// ------------------------------------------------------------------------------------------------
// A declaration: each function's call carries the capabilities it serves

export const acme = adapter({
  requests: ["image.generate", "structured.generate"],
  jobs: ["video.generate"],
  send: async (call) => {
    expectType<Equal<typeof call.capability, "image.generate" | "structured.generate">>(true);
    expectType<Equal<typeof call.handle, undefined>>(true);
    expectType<string>(call.baseUrl);
    expectType<number>(call.attempt);
    expectType<readonly number[]>(call.takes);
    expectType<ReadonlyMap<string, AdapterFile>>(call.files);
    expectType<AdapterValue | undefined>(call.request.prompt);
    expectType<JsonValue | undefined>(call.route.contract.adapter);
    expectType<AbortSignal>(call.signal);
    if (call.capability === "structured.generate") {
      return Answered.json({ name: "teal" }, { costUsd: 0.01, usage: { inputTokens: 5 } });
    }
    let response: FetchResponse;
    try {
      response = await call.fetch(`${call.baseUrl}/draw`, {
        method: "POST",
        json: { prompt: call.request.prompt instanceof Object ? null : call.request.prompt },
        headers: { "x-acme": "1" },
        credential: true,
        timeoutS: 60,
      });
    } catch (error) {
      if (error instanceof FetchError) {
        expectType<"refused" | "not_sent" | "after_send">(error.phase);
        return error.phase === "not_sent"
          ? { outcome: "not_received", reason: error.reason, retryAfterS: 1 }
          : { outcome: "failed", reason: error.reason, retryable: error.phase === "after_send" };
      }
      throw error;
    }
    expectType<number>(response.status);
    expectType<readonly (readonly [string, string])[]>(response.headers);
    expectType<unknown>(response.json());
    return {
      outcome: "answered",
      files: { image: response.body, again: { bytes, kind: "image/png" }, path: new URL("file:///tmp/a.png") },
      data: null,
      costUsd: undefined,
    };
  },
  submit: (call) => {
    expectType<Equal<typeof call.capability, "video.generate">>(true);
    return call.attempt > 1 ? { outcome: "uncertain", reason: "lost" } : { outcome: "accepted", handle: { id: call.key } };
  },
  collect: async (call) => {
    expectType<Equal<typeof call.handle, JobHandle>>(true);
    const id = call.handle.id;
    if (id === undefined) {
      return { outcome: "unreachable", reason: "no id" };
    }
    return call.take > 1 ? { outcome: "ended", reason: "cancelled", costUsd: 0 } : { outcome: "answered", data: { id } };
  },
  version: 1,
});
expectType<Adapter<"image.generate" | "structured.generate", "video.generate">>(acme);
expectType<Adapter>(acme);
expectType<readonly ("image.generate" | "structured.generate")[]>(acme.requests);
expectType<number | null>(acme.version);

// Functions declared on their own take the general call, and say their outcomes.
async function send(call: AdapterCall): Promise<SendOutcome> {
  return { outcome: "refused", reason: `no ${call.capability} today` };
}
function submit(call: AdapterCall): SubmitOutcome {
  return { outcome: "accepted", handle: { key: call.key, takes: call.takes } };
}
async function collect(call: CollectCall): Promise<CollectOutcome> {
  return { outcome: "answered", data: { job: call.handle.key ?? null } };
}
export const plain = adapter({ requests: ["image.generate"], jobs: ["acme.render"], send, submit, collect });
export const sendsOnly = adapter({ requests: ["image.generate"], send, version: null });
export const jobsOnly = adapter({ jobs: ["acme.render"], submit, collect, version: undefined });
const names: string[] = ["image.generate"];
export const listed = adapter({ requests: names, send });
const accepted: Accepted = { outcome: "accepted", handle: { id: "j-1", note: undefined } };
void accepted;

// ------------------------------------------------------------------------------------------------
// What must not compile

// @ts-expect-error an adapter serves at least one capability
adapter({});
// @ts-expect-error an adapter serves at least one capability
adapter({ requests: [] });
// @ts-expect-error requests need send
adapter({ requests: ["image.generate"] });
// @ts-expect-error jobs need submit and collect
adapter({ jobs: ["acme.render"], submit });
// @ts-expect-error a field adapter() does not know
adapter({ requests: ["image.generate"], send, versoin: 1 });
adapter({
  requests: ["image.generate"],
  // @ts-expect-error send returns what a send takes, not a job's acceptance
  send: () => ({ outcome: "accepted", handle: {} }),
});
adapter({
  requests: ["image.generate"],
  // @ts-expect-error an outcome's members are named in camelCase
  send: () => ({ outcome: "answered", cost_usd: 1 }),
});
adapter({
  requests: ["image.generate"],
  // @ts-expect-error a failure says whether it may be tried again
  send: async () => ({ outcome: "failed", reason: "acme broke" }),
});
adapter({
  requests: ["image.generate"],
  // @ts-expect-error a refusal costs nothing: costUsd is an answer's or a failure's
  send: async (call) => {
    if (call.attempt > 1) {
      return { outcome: "answered", data: null, costUsd: 0.01 };
    }
    return { outcome: "refused", reason: "x", costUsd: 0.01 };
  },
});
adapter({
  jobs: ["acme.render"],
  submit,
  // @ts-expect-error collect never refuses: the job was accepted
  collect: () => ({ outcome: "refused", reason: "no" }),
});
adapter({
  requests: ["image.generate"],
  send: (call) => {
    // @ts-expect-error a request has no handle
    call.handle.id;
    // @ts-expect-error a call is for the capabilities its function serves
    const other: "video.generate" = call.capability;
    void other;
    return { outcome: "refused", reason: "x" };
  },
});
adapter({
  requests: ["image.generate"],
  send: async (call) => {
    // @ts-expect-error fetch takes GET or POST
    await call.fetch("https://api.acme.test", { method: "PUT" });
    // @ts-expect-error a body is bytes
    await call.fetch("https://api.acme.test", { method: "POST", body: "text", contentType: "text/plain" });
    // @ts-expect-error an option fetch does not know
    await call.fetch("https://api.acme.test", { verb: "GET" });
    // @ts-expect-error a lane fetch does not know
    await call.fetch("https://api.acme.test", { lane: "cdn" });
    return { outcome: "refused", reason: "x" };
  },
});
// @ts-expect-error an answer's file is bytes, an AdapterFile, {bytes, kind} or a file URL
export const wrongFile: SendOutcome = { outcome: "answered", files: { image: "picture.png" } };
// @ts-expect-error a handle holds JSON values
export const wrongHandle: SubmitOutcome = { outcome: "accepted", handle: { at: new Date() } };
