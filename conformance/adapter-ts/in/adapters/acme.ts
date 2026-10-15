// A synthetic adapter for the made-up provider acme: it answers from what it is asked and makes
// no request. Its one fetch is refused before it leaves, since a conformance step runs with the
// network off. `acme` answers at once; `acmeJobs` submits a long job and collects it.

import { Answered, adapter, FetchError } from "@grida/fx";
import { ASKED, WAITED_FOR } from "./colours.ts";

export const acme = adapter({
  requests: ["structured.generate"],
  send: async (call) => {
    if (call.request.prompt === "refuse") {
      // The key in a reason reaches no record: the engine redacts it.
      return { outcome: "refused", reason: `no colours for ${process.env.ACME_API_KEY}` };
    }
    let fetched: string[] | null = null;
    try {
      await call.fetch(`${call.baseUrl}/chat`, { method: "POST", json: {}, credential: true });
    } catch (error) {
      if (!(error instanceof FetchError)) {
        throw error;
      }
      fetched = [error.phase, error.reason];
    }
    const answer = {
      name: ASKED,
      fetched,
      contract: call.route.contract,
      attempt: call.attempt,
      key_given: "ACME_API_KEY" in process.env,
    };
    return Answered.json(answer, { costUsd: 0.01, usage: { inputTokens: 5, outputTokens: 3 } });
  },
  version: 1,
});

export const acmeJobs = adapter({
  jobs: ["structured.generate"],
  submit: (call) => ({ outcome: "accepted", handle: { job: call.key.slice(0, 12) } }),
  collect: async (call) => {
    const answer = {
      name: WAITED_FOR,
      job: call.handle.job ?? null,
      contract: call.route.contract,
      key_given: "ACME_API_KEY" in process.env,
    };
    return Answered.json(answer, { costUsd: 0.02 });
  },
  version: 1,
});
