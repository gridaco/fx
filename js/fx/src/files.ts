/**
 * Files in a run: {@link InputFile}, a file the engine handed the run, and {@link Output}, a file
 * the body made with `ctx.out`; and {@link AdapterFile}, a file a project adapter's call names.
 *
 * Each carries a brand (`Symbol.for`), so the host recognises them whichever copy of `@grida/fx`
 * made them. An output leaves the host only when it is returned, put in a request, or shown to an
 * agent (`spec/protocol.md` §3.4, §6.7).
 */

import { readFile } from "node:fs/promises";
import { brandOf } from "./protocol.js";
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

/** The brand of an {@link AdapterFile}: where its bytes are, and its kind. */
export const ADAPTER_FILE: unique symbol = Symbol.for("grida.fx.adapter_file") as never;

/** What an {@link AdapterFile}'s brand holds. */
export interface AdapterFileRecord {
  readonly path: string;
  readonly kind: string;
}

/**
 * A file a project adapter's call names (`spec/protocol.md` §5.8): its `digest`, `kind` and
 * `size`, and the `path` of its store copy, which an adapter reads and never writes, moves or
 * deletes. An adapter may answer with it as one of its files, which keeps its kind.
 */
export class AdapterFile {
  readonly digest: string;
  readonly kind: string;
  readonly size: number;
  readonly path: string;
  declare readonly [ADAPTER_FILE]: AdapterFileRecord;

  /** Made by the host from an `adapter_file` the engine sent. */
  constructor(file: {
    readonly digest: string;
    readonly kind: string;
    readonly size: number;
    readonly path: string;
  }) {
    this.digest = String(file.digest);
    this.kind = String(file.kind);
    this.size = Number(file.size);
    this.path = String(file.path);
    Object.defineProperty(this, ADAPTER_FILE, { value: Object.freeze({ path: this.path, kind: this.kind }) });
  }

  /** The file's bytes. */
  async bytes(): Promise<Uint8Array> {
    return readFile(this.path);
  }

  toString(): string {
    return `AdapterFile(${this.digest}, ${this.kind})`;
  }
}

/** Where an {@link AdapterFile} of any copy of `@grida/fx` is, and its kind; undefined for
 * anything else. */
export function adapterFileOf(value: unknown): AdapterFileRecord | undefined {
  const found = objectBrand(value, ADAPTER_FILE) as Partial<AdapterFileRecord> | undefined;
  return typeof found?.path === "string" && typeof found.kind === "string"
    ? (found as AdapterFileRecord)
    : undefined;
}

/** The file ref of an {@link InputFile} of any copy of `@grida/fx`; undefined for anything
 * else. */
export function fileRefOf(value: unknown): FileRefWire | undefined {
  return objectBrand(value, INPUT_FILE) as FileRefWire | undefined;
}

/** Where an {@link Output} of any copy of `@grida/fx` is; undefined for anything else. */
export function outputSourceOf(value: unknown): OutputSource | undefined {
  return objectBrand(value, OUTPUT) as OutputSource | undefined;
}

/** The object an object holds under the brand `mark`; undefined for anything else. */
function objectBrand(value: unknown, mark: symbol): object | undefined {
  const found = typeof value === "object" ? brandOf(value, mark) : undefined;
  return typeof found === "object" && found !== null ? found : undefined;
}
