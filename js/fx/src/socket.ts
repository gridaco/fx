/**
 * A connected pair of Unix-domain stream sockets, for a stand-in run: the engine's standard input
 * is one end (`grida-fx run --stand-in -`), and the stand-in is served on the other
 * (`spec/protocol.md` §5.7, §8.2).
 *
 * Node has no `socketpair()`, so the pair is made the other way:
 * 1. listen on a socket in a fresh private folder (`mkdtemp`, mode 0700);
 * 2. connect to it, and accept that connection;
 * 3. close the listener and remove the folder.
 *
 * The accepted end is paused from the start: nothing here ever reads what the engine is sent.
 */

import { mkdtemp, rm } from "node:fs/promises";
import { createConnection, createServer, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

/** The longest path a Unix-domain socket takes, less its terminating NUL (`sun_path`: 108 bytes
 * on Linux, 104 on macOS and the BSDs). */
const SOCKET_PATH_MAX = process.platform === "linux" ? 107 : 103;
const PREFIX = "grida-fx-stand-in-";
/** The socket's name in its folder. */
const NAME = "s";

/** The folder the private folder is made in: the system's temporary folder, or `/tmp` when a
 * socket path there would be too long (a long `TMPDIR`). */
export function socketRoot(): string {
  const root = tmpdir();
  // mkdtemp adds six characters.
  const longest = join(root, `${PREFIX}XXXXXX`, NAME);
  return Buffer.byteLength(longest) <= SOCKET_PATH_MAX ? root : "/tmp";
}

/** A connected pair (module comment): `ours` to serve on, `theirs` to hand to the engine. */
export async function socketPair(): Promise<{ ours: Socket; theirs: Socket }> {
  const folder = await mkdtemp(join(socketRoot(), PREFIX));
  const path = join(folder, NAME);
  const server = createServer({ pauseOnConnect: true });
  server.maxConnections = 1;
  try {
    const accepted = new Promise<Socket>((resolve, reject) => {
      server.once("connection", resolve);
      server.once("error", reject);
    });
    // Awaited below; a listener that fails first rejects it unread.
    accepted.catch(() => undefined);
    await new Promise<void>((resolve, reject) => {
      server.once("error", reject);
      server.listen(path, () => resolve());
    });
    const ours = createConnection(path);
    try {
      await new Promise<void>((resolve, reject) => {
        ours.once("connect", () => resolve());
        ours.once("error", reject);
      });
      return { ours, theirs: await accepted };
    } catch (error) {
      ours.destroy();
      throw error;
    }
  } finally {
    // Not waited for: a listener closes only once its connections have ended.
    server.close();
    await rm(folder, { recursive: true, force: true });
  }
}
