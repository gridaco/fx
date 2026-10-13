/**
 * The TypeScript node host's first module: `<node> <@grida/fx>/dist/host-entry.js`
 * (`spec/protocol.md` §1).
 *
 * It imports nothing but `node:fs`, so no other code runs before the protocol streams are moved
 * out of user code's reach:
 *
 * 1. the protocol output becomes a descriptor of its own (`/dev/fd/1` opened anew), and descriptor
 *    1 is reopened on stderr: `console.log`, `process.stdout` and every program user code starts
 *    with an inherited stdout write to stderr;
 * 2. the protocol input becomes a descriptor of its own (`/dev/fd/0`), and descriptor 0 is
 *    reopened on the null device, so user code and its programs never read protocol bytes;
 * 3. writes to `process.stdout` and `process.stderr` finish before they return when these are
 *    pipes, so nothing user code printed is still queued in the process when the host ends.
 *
 * Descriptors Node opens are close-on-exec, so no program user code starts holds the protocol
 * streams. A host that cannot open one of these anew (a stderr that is a socket, which Linux does
 * not open again through `/proc`; the engine gives a TypeScript host a pipe) refuses to start:
 * a sentence on stderr, status 70.
 *
 * `process.exit` then belongs to the host alone: user code that calls it gets an error, as a body
 * that threw would. The host ends with `process.reallyExit`, which no `exit` listener can stop and
 * which flushes nothing: the protocol stream and the host's own log lines are written with
 * `writeSync`, and user output by step 3, so nothing is left to flush.
 */

import { closeSync, constants, openSync, writeSync } from "node:fs";

/** Refuses to start: a sentence on stderr, status 70. */
function refuse(text: string): never {
  try {
    writeSync(2, `@grida/fx node host: ${text}\n`);
  } catch {
    // no stderr either
  }
  process.exit(70);
}

/** Opens `path` anew, or null when it cannot be. */
function open(path: string, flags: number): number | null {
  try {
    return openSync(path, flags);
  } catch {
    return null;
  }
}

/** Requires `found` to be `expected`, the lowest descriptor free. */
function landed(found: number | null, expected: number, what: string): void {
  if (found !== expected) {
    refuse(`${what} could not become descriptor ${expected}`);
  }
}

const protocolOut = open("/dev/fd/1", constants.O_WRONLY);
if (protocolOut === null) {
  refuse(
    "cannot keep the protocol stream apart from user output: " +
      "standard output is not a pipe it can open again",
  );
}
closeSync(1);
landed(open("/dev/fd/2", constants.O_WRONLY | constants.O_APPEND), 1, "stderr");
const protocolIn = open("/dev/fd/0", constants.O_RDONLY);
if (protocolIn === null) {
  refuse(
    "cannot keep the protocol input apart from user code: " +
      "standard input is not a pipe it can open again",
  );
}
closeSync(0);
landed(open("/dev/null", constants.O_RDONLY), 0, "the null device");

// Writes to `process.stdout` and `process.stderr` finish before they return when these are pipes
// (Node queues them otherwise, outside Linux), as Python's do: the host ends with
// `process.reallyExit`, which would drop what is queued, and the engine's copy may read the pipe
// more slowly than user code prints. This also clears O_NONBLOCK on the pipe, which every program
// user code starts inherits, and `log`'s `writeSync(2)` then waits for room too.
for (const stream of [process.stdout, process.stderr]) {
  const handle = (stream as { _handle?: { setBlocking?: (blocking: boolean) => unknown } })._handle;
  try {
    handle?.setBlocking?.(true);
  } catch {
    // stays as Node made it
  }
}

/** Ends the process at once: an `exit` listener that throws cannot stop it (`process.exit` emits
 * `exit` first, and a throw there skips the end). */
const reallyExit = (process as unknown as { reallyExit?: (status: number) => never }).reallyExit;
const processExit = process.exit.bind(process) as (status: number) => never;
const exit = (status: number): never => {
  if (typeof reallyExit === "function") {
    reallyExit.call(process, status);
  }
  try {
    return processExit(status);
  } finally {
    process.kill(process.pid, "SIGKILL");
  }
};
process.exit = ((code?: number | string | null) => {
  throw new Error(`process.exit(${code === undefined ? "" : String(code)}) cannot end the node host`);
}) as typeof process.exit;

const { main } = await import("./host.js");
main(protocolIn, protocolOut, exit);
