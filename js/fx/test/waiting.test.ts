import { expect, test } from "bun:test";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { AnswerError, inspectRequests, listRequests, respond, run } from "../src/index.js";
import { SLOW, temporaryFolder, testEngine } from "./support.js";

const engine = testEngine();

test.skipIf(engine === null)("answer another invocation and recover the recorded SDK decision", async () => {
  const folder = temporaryFolder("sdk-waiting");
  try {
    const options = {
      cwd: folder.path,
      env: { GRIDA_FX_BIN: engine!, GRIDA_FX_NETWORK: "off", GRIDA_FX_DISABLE_DOTENV: "1" },
    };
    writeFileSync(join(folder.path, "fx.yaml"), "fx: project/v1\n");
    writeFileSync(join(folder.path, "workflow.yaml"), JSON.stringify({
      fx: "workflow/v1", id: "decision", title: "Review",
      steps: {
        gate: { uses: "fx/await@1", with: {
          message: "Approve this step?", schema: { type: ["boolean", "null"] }, timeout_seconds: 20,
        } },
        revision: { uses: "fx/await@1", with: { schema: { type: "string" } } },
      },
      outputs: { result: "${{ steps.gate.outputs.answer }}", revision: "${{ steps.revision.outputs.answer }}" },
    }));
    const selector = "decision/review";
    const parked = await run("workflow.yaml", { ...options, name: "review" });
    expect(parked.incomplete).toBe(true);
    expect(parked.inspection.run.state).toBe("awaiting");
    const requests = await listRequests(selector, options);
    const request = requests.find(item => item.step === "gate#1");
    const revision = requests.find(item => item.step === "revision#1");
    expect(request!.state.status).toBe("pending");
    expect(revision!.schema).toEqual({ type: "string" });
    expect(revision).not.toHaveProperty("message");
    expect((await inspectRequests(selector, options)).viewer_url).toBeNull();
    expect((await inspectRequests(selector, { ...options, register: true })).viewer_url).toBeNull();
    await respond(selector, revision!.id, "build-104", options);
    const execution = run("workflow.yaml", { ...options, resume: "review", waitForAnswers: true });
    const winner = await respond(selector, request!.id, null, options);
    expect(winner.state).toMatchObject({ status: "answered", channel: "sdk", answer: null });
    expect(await respond(selector, request!.id, null, options)).toEqual(winner);
    const conflict = await respond(selector, request!.id, false, options).catch(error => error);
    expect(conflict).toBeInstanceOf(AnswerError);
    expect(conflict.code).toBe("answer_conflict");
    expect(conflict.request).toEqual(winner);
    const result = await execution;
    expect(result.ok).toBe(true);
    expect(result.outputs.revision).toBe("build-104");
  } finally {
    folder.cleanup();
  }
}, SLOW);
