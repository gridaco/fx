import { expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const script = join(import.meta.dir, "ignore-build.sh");
const root = join(import.meta.dir, "../..");
const person = { GIT_AUTHOR_NAME: "acme", GIT_AUTHOR_EMAIL: "acme@example.com", GIT_COMMITTER_NAME: "acme", GIT_COMMITTER_EMAIL: "acme@example.com" };
const env = (home: string, vercel: Record<string, string> = {}) => ({ PATH: process.env.PATH ?? "", HOME: home, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null", ...person, ...vercel });

function git(home: string, cwd: string, ...args: string[]): string {
  const run = Bun.spawnSync(["git", ...args], { cwd, env: env(home) });
  if (run.exitCode !== 0) throw new Error(`git ${args.join(" ")}: ${run.stderr.toString()}`);
  return run.stdout.toString().trim();
}

// main: c1 adds the website, c2 changes it, c3 to c5 change the engine. Like Vercel's, the
// clone is shallower than the history: it holds c4 and c5, and an older deployment is fetched.
async function deployments(check: (tools: { commits: string[]; inClone: (...args: string[]) => string; commit: (path: string) => Promise<string>; decide: (vercel?: Record<string, string>) => number }) => Promise<void>): Promise<void> {
  const home = await mkdtemp(join(await realpath(tmpdir()), "fx-ignore-build-"));
  try {
    const origin = join(home, "origin"), clone = join(home, "clone");
    let edits = 0;
    const write = async (repo: string, path: string) => {
      await mkdir(dirname(join(repo, path)), { recursive: true });
      await writeFile(join(repo, path), `${path} ${++edits}\n`);
      git(home, repo, "add", "-A");
      git(home, repo, "commit", "--quiet", "-m", path);
      return git(home, repo, "rev-parse", "HEAD");
    };
    await mkdir(origin);
    git(home, origin, "init", "--quiet", "-b", "main");
    const commits: string[] = [];
    for (const path of ["www/page.txt", "www/page.txt", "crates/core.rs", "crates/core.rs", "crates/core.rs"]) commits.push(await write(origin, path));
    git(home, home, "clone", "--quiet", "--depth=2", `file://${origin}`, clone);
    const decide = (vercel: Record<string, string> = {}) => Bun.spawnSync(["sh", script], { cwd: join(clone, "www"), env: env(home, vercel), stdout: "pipe", stderr: "pipe" }).exitCode;
    await check({ commits, inClone: (...args) => git(home, clone, ...args), commit: path => write(clone, path), decide });
  } finally { await rm(home, { recursive: true, force: true }); }
}

test("a deployment skips when nothing the website is built from changed since its branch's last", async () => {
  await deployments(async ({ commits, decide }) => {
    expect(decide({ VERCEL_GIT_PREVIOUS_SHA: commits[2] })).toBe(0);
  });
});

test("a deployment builds when the website changed since the last, or that deployment cannot be found", async () => {
  await deployments(async ({ commits, decide }) => {
    expect(decide({ VERCEL_GIT_PREVIOUS_SHA: commits[0] })).toBe(1);
    expect(decide({ VERCEL_GIT_PREVIOUS_SHA: "0".repeat(40) })).toBe(1);
  });
});

test("a branch's first deployment compares the website with main's, keeping the clone's history; production's always builds", async () => {
  await deployments(async ({ inClone, commit, decide }) => {
    inClone("switch", "--quiet", "-c", "acme");
    await commit("crates/core.rs");
    expect(decide({ VERCEL_ENV: "preview" })).toBe(0);
    expect(inClone("rev-list", "--count", "HEAD")).toBe("3");
    expect(decide({ VERCEL_ENV: "production" })).toBe(1);
    expect(decide()).toBe(1);
    await commit("web/fx-web/src/index.ts");
    expect(decide({ VERCEL_ENV: "preview" })).toBe(1);
  });
});

test("the build watches www, every workspace package it imports and the workspace install", async () => {
  const manifest = async (folder: string) => JSON.parse(await readFile(join(root, folder, "package.json"), "utf8")) as { name: string; workspaces?: string[]; dependencies?: Record<string, string> };
  const folders = new Map<string, string>();
  for (const folder of (await manifest(".")).workspaces ?? []) folders.set((await manifest(folder)).name, folder);
  const watched = new Set(["www", "package.json", "bun.lock"]);
  for (const queue = ["www"]; queue.length;) {
    for (const [name, version] of Object.entries((await manifest(queue.pop()!)).dependencies ?? {})) {
      const folder = folders.get(name);
      if (version.startsWith("workspace:") && folder && !watched.has(folder)) { watched.add(folder); queue.push(folder); }
    }
  }
  const listed = /^website="([^"]+)"$/m.exec(await readFile(script, "utf8"))?.[1].split(" ");
  expect(new Set(listed)).toEqual(watched);
});
