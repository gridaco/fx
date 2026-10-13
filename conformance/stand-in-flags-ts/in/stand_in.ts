// The stand-in of the stand-in-flags-ts case: it counts the call in `count.txt`, in its working
// directory (the project the case runs in), and then breaks, as a test harness with a bug would.
// A stand-in that throws anything but CallRefused or CallFailed is a fault of the stand-in, and
// the engine stops the run.

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import type { StandInCall } from "@grida/fx";

const COUNT = "count.txt";

export function answer(call: StandInCall): never {
  const count = existsSync(COUNT) ? Number(readFileSync(COUNT, "utf8")) : 0;
  writeFileSync(COUNT, `${count + 1}\n`);
  throw new Error(`the test harness broke on ${call.capability}`);
}
