// The grida-fx command as npm installs it: bin/grida-fx.js over the compiled dist, run by Node,
// with a stand-in engine (a shell script) in GRIDA_FX_BIN. Skipped without Node or on Windows.

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { packageRoot, SLOW, temporaryFolder } from "./support.js";

const node = spawnSync("node", ["--version"]).status === 0 && process.platform !== "win32";

/** The stand-in engine: what it does is its first argument. */
const FAKE_ENGINE = `#!/bin/sh
log="$FAKE_LOG"
case "$1" in
  args) shift; for a in "$@"; do printf '%s\\n' "$a"; done; exit 0 ;;
  exit) exit "$2" ;;
  kill) kill -"$2" $$; sleep 5; exit 99 ;;
  ids) echo "$$ $(ps -o pgid= -p $$ | tr -d ' ')"; exit 0 ;;
  wait)
    trap 'echo INT >> "$log"' INT
    trap 'echo TERM >> "$log"; exit 43' TERM
    echo "ready $$" >> "$log"
    i=0
    while [ $i -lt 400 ]; do sleep 0.05; i=$((i + 1)); done
    exit 98 ;;
esac
exit 97
`;

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

describe.skipIf(!node)("the grida-fx command", () => {
  const folder = temporaryFolder("cli");
  const shim = join(folder.path, "bin", "grida-fx.js");
  const fake = join(folder.path, "fake-engine");
  const log = join(folder.path, "log.txt");
  const env = { ...process.env, GRIDA_FX_BIN: fake, FAKE_LOG: log };

  beforeAll(() => {
    const tsc = join(packageRoot, "node_modules", "typescript", "bin", "tsc");
    const built = spawnSync(
      "node",
      [tsc, "-p", "tsconfig.build.json", "--outDir", join(folder.path, "dist")],
      {
        cwd: packageRoot,
        encoding: "utf8",
      },
    );
    expect(built.stdout + built.stderr).toBe("");
    mkdirSync(join(folder.path, "bin"));
    copyFileSync(join(packageRoot, "bin", "grida-fx.js"), shim);
    writeFileSync(join(folder.path, "package.json"), JSON.stringify({ type: "module" }));
    writeFileSync(fake, FAKE_ENGINE);
    chmodSync(fake, 0o755);
  }, SLOW);
  afterAll(() => folder.cleanup());

  const runShim = (args: string[], extra: Record<string, string> = {}) =>
    spawnSync("node", [shim, ...args], { env: { ...env, ...extra }, encoding: "utf8" });

  /** Starts the command on the waiting stand-in, once the stand-in is ready. */
  async function waiting(): Promise<ChildProcess> {
    writeFileSync(log, "");
    const child = spawn("node", [shim, "wait"], { env, stdio: "ignore" });
    for (let i = 0; i < 200 && !readFileSync(log, "utf8").includes("ready"); i += 1) {
      await sleep(25);
    }
    expect(readFileSync(log, "utf8")).toContain("ready");
    return child;
  }

  const ended = (child: ChildProcess) =>
    new Promise<{ code: number | null; signal: string | null }>((done) =>
      child.once("exit", (code, signal) => done({ code, signal })),
    );

  const state = (pid: number) =>
    spawnSync("ps", ["-o", "stat=", "-p", String(pid)], { encoding: "utf8" }).stdout.trim();

  test(
    "arguments and output pass through",
    () => {
      const done = runShim(["args", "a b", "--x=y", "", "é 😀", "-"]);
      expect(done.status).toBe(0);
      expect(done.stdout).toBe("a b\n--x=y\n\né 😀\n-\n");
    },
    SLOW,
  );

  test(
    "the exit status passes through",
    () => {
      expect(runShim(["exit", "3"]).status).toBe(3);
      expect(runShim(["exit", "130"]).status).toBe(130);
      expect(runShim(["exit", "0"]).status).toBe(0);
    },
    SLOW,
  );

  test(
    "an engine ended by a signal ends the command by the same signal",
    () => {
      expect(runShim(["kill", "TERM"]).signal).toBe("SIGTERM");
      expect(runShim(["kill", "KILL"]).signal).toBe("SIGKILL");
    },
    SLOW,
  );

  test(
    "the engine runs in a process group of its own",
    () => {
      const done = runShim(["ids"]);
      const [pid, pgid] = done.stdout.trim().split(" ");
      expect(pgid).toBe(pid);
    },
    SLOW,
  );

  test(
    "no engine: a message and status 1",
    () => {
      const done = runShim(["--version"], { GRIDA_FX_BIN: join(folder.path, "nowhere") });
      expect(done.status).toBe(1);
      expect(done.stderr).toBe(
        `grida-fx: GRIDA_FX_BIN is ${join(folder.path, "nowhere")}, which is not a file\n`,
      );
    },
    SLOW,
  );

  test(
    "SIGINT and SIGTERM reach the engine, once per interruption",
    async () => {
      const child = await waiting();
      const exit = ended(child);
      // A repeat within the window (a launcher passing on the terminal's Ctrl-C) is one interruption.
      child.kill("SIGINT");
      await sleep(100);
      child.kill("SIGINT");
      await sleep(600);
      // A later one is another.
      child.kill("SIGINT");
      await sleep(300);
      child.kill("SIGTERM");
      expect(await exit).toEqual({ code: 43, signal: null });
      expect(
        readFileSync(log, "utf8")
          .split("\n")
          .filter((line) => line && !line.startsWith("ready")),
      ).toEqual(["INT", "INT", "TERM"]);
    },
    SLOW,
  );

  test(
    "Ctrl-Z stops the engine with the command, and SIGCONT continues both",
    async () => {
      const child = await waiting();
      const exit = ended(child);
      const engine = Number(readFileSync(log, "utf8").match(/ready (\d+)/)?.[1]);
      child.kill("SIGTSTP");
      await sleep(300);
      expect(state(child.pid as number)).toStartWith("T");
      expect(state(engine)).toStartWith("T");
      child.kill("SIGCONT");
      await sleep(300);
      expect(state(child.pid as number)).not.toStartWith("T");
      expect(state(engine)).not.toStartWith("T");
      child.kill("SIGTERM");
      expect(await exit).toEqual({ code: 43, signal: null });
    },
    SLOW,
  );

  test(
    "the bin script is executable and calls the compiled CLI",
    () => {
      const text = readFileSync(join(packageRoot, "bin", "grida-fx.js"), "utf8");
      expect(text.startsWith("#!/usr/bin/env node\n")).toBe(true);
      expect(text).toContain('from "../dist/cli.js"');
      expect(existsSync(join(folder.path, "dist", "cli.js"))).toBe(true);
    },
    SLOW,
  );
});
