/**
 * Files in a run: {@link InputFile}, a file the engine handed the run, and {@link Output}, a file
 * the body made with `ctx.out`.
 *
 * Both carry a brand (`Symbol.for`), so the host recognises them whichever copy of `@grida/fx`
 * made them. An output leaves the host only when it is returned, put in a request, or shown to an
 * agent (`spec/protocol.md` §3.4, §6.7).
 */

import { readFile } from "node:fs/promises";
import type { JsonValue } from "./workflow.js";

/** The brand of an {@link InputFile}: its file ref, as the engine sent it. */
export const INPUT_FILE: unique symbol = Symbol.for("grida.fx.input_file") as never;
/** The brand of an {@link Output}: where its bytes are. */
export const OUTPUT: unique symbol = Symbol.for("grida.fx.output") as never;

/** A file digest: what a file value holds (`spec/identity.md` §3). */
export const DIGEST = /^[0-9a-f]{64}$/;

/** A file ref as the engine sends it (`spec/protocol.md` §3.1). */
export interface FileRefWire {
  readonly digest: string;
  readonly kind: string;
  readonly size: number;
  readonly name: string;
  readonly key?: string | null;
  readonly path: string;
  readonly facts: { readonly [name: string]: JsonValue };
}

/**
 * A file the engine handed the run: an input, a param file, or a file of a capability's answer.
 * `path` is its copy in the store: read it, never write it. `facts` are the file facts the engine
 * computed (`spec/identity.md` §4): `bytes` and `kind`, with `width`, `height`, `has_alpha` and
 * `opaque` for a picture. Return it as an output to pass the file through.
 */
export class InputFile {
  readonly digest: string;
  readonly kind: string;
  readonly size: number;
  /** A display name; never part of an identity. */
  readonly name: string;
  /** The item's key in a keyed collection; null otherwise. */
  readonly key: string | null;
  readonly path: string;
  readonly facts: { readonly [name: string]: JsonValue };
  declare readonly [INPUT_FILE]: FileRefWire;
  readonly #ref: FileRefWire;

  /** Made by the host from a file ref the engine sent. */
  constructor(ref: FileRefWire) {
    this.#ref = ref;
    this.digest = String(ref.digest);
    this.kind = String(ref.kind);
    this.size = Number(ref.size);
    this.name = typeof ref.name === "string" ? ref.name : "";
    this.key = typeof ref.key === "string" ? ref.key : null;
    this.path = String(ref.path);
    this.facts = Object.freeze({ ...(ref.facts ?? {}) });
    Object.defineProperty(this, INPUT_FILE, { value: ref });
  }

  /** The file's bytes. */
  async bytes(): Promise<Uint8Array> {
    return readFile(this.#ref.path);
  }

  /** The file as text: strict UTF-8, one leading byte-order mark dropped, line endings kept. */
  async text(): Promise<string> {
    return decodeText(await this.bytes());
  }

  /** The file as JSON. */
  async json(): Promise<unknown> {
    return JSON.parse(await this.text()) as unknown;
  }

  toString(): string {
    return `InputFile(${this.name || this.digest}, ${this.kind})`;
  }
}

/** UTF-8 text, strictly decoded, without one leading byte-order mark (`spec/identity.md` §5). */
export function decodeText(bytes: Uint8Array): string {
  return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
}

/** Where an output's bytes are: a file under the run's work folder (POSIX, relative to it), bytes
 * in memory, or a JSON value the engine writes in its canonical format. */
export type OutputSource =
  | { readonly workPath: string }
  | { readonly data: Uint8Array }
  | { readonly json: JsonValue };

/** A file the body made, from `ctx.out`: return it as an output, put it in a capability request,
 * or show it to an agent. */
export interface Output {
  /** Its file kind (`image/png`, `json`, …). */
  readonly kind: string;
  readonly [OUTPUT]: OutputSource;
}

/** Made by `ctx.out` only. */
class MadeOutput implements Output {
  readonly kind: string;
  declare readonly [OUTPUT]: OutputSource;

  constructor(kind: string, source: OutputSource) {
    this.kind = kind;
    Object.defineProperty(this, OUTPUT, { value: source });
  }

  toString(): string {
    return `Output(${this.kind})`;
  }
}

/** A new {@link Output}. */
export function makeOutput(kind: string, source: OutputSource): Output {
  return new MadeOutput(kind, source);
}

/** The file ref of an {@link InputFile} of any copy of `@grida/fx`; undefined for anything
 * else. */
export function fileRefOf(value: unknown): FileRefWire | undefined {
  return brandOf(value, INPUT_FILE) as FileRefWire | undefined;
}

/** Where an {@link Output} of any copy of `@grida/fx` is; undefined for anything else. */
export function outputSourceOf(value: unknown): OutputSource | undefined {
  return brandOf(value, OUTPUT) as OutputSource | undefined;
}

function brandOf(value: unknown, mark: symbol): unknown {
  if (typeof value !== "object" || value === null) {
    return undefined;
  }
  try {
    const found = (value as Record<symbol, unknown>)[mark];
    return typeof found === "object" && found !== null ? found : undefined;
  } catch {
    return undefined;
  }
}
