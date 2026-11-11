import { appendFileSync, existsSync, writeFileSync } from "node:fs";
import { setTimeout } from "node:timers/promises";
import { adapter, AdapterFile, type AdapterDiagnostic } from "@grida/fx";

export const native_adapter = adapter({
  jobs: ["acme.native"], version: 1,
  submit: async (call) => {
    const source = call.request.source as AdapterFile;
    const text = Buffer.from(await source.bytes()).toString("utf8");
    appendFileSync(".native-submits", JSON.stringify({ key: call.key, operation: call.request.operation,
      future: call.request.future_face_budget, vendor_extension: call.request.vendor_extension, source: text }) + "\n");
    const mode = call.request.mode;
    if (mode === "reject" || mode === "uncertain") {
      const diagnostic: AdapterDiagnostic = { source: "provider", phase: "submit",
        operation: call.request.operation as string, httpStatus: mode === "reject" ? 400 : 502,
        code: "bad_option", field: "future_face_budget", requestId: "req-native-1", detail: "available",
        message: "Unsupported future_face_budget; " + process.env.ACME_NATIVE_KEY
          + " https://assets.acme.test/a?signature=made-up-signature /private/native/input.txt " + text };
      if (mode === "uncertain") return { outcome: "uncertain", reason: "Provider receipt is unknown", diagnostic };
      return { outcome: "failed", reason: "Provider rejected the input", retryable: false, diagnostic };
    }
    return { outcome: "accepted", handle: { task_id: "task-" + call.key.slice(0, 16) } };
  },
  collect: async (call) => {
    writeFileSync(".native-collecting", "");
    while (!existsSync(".native-release")) await setTimeout(20, undefined, { signal: call.signal });
    return { outcome: "answered", data: { json: { task_id: call.handle.task_id,
      future: call.request.future_face_budget, operation: call.request.operation } }, costUsd: 0.01 };
  },
});
