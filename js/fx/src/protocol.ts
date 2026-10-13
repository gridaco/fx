/**
 * The node protocol's transport (`spec/protocol.md` §1), shared by the TypeScript node host and
 * the stand-in answerer.
 *
 * Framing is LSP's base protocol: `Content-Length: <n>\r\n\r\n`, then n bytes of UTF-8 JSON.
 * Messages are JSON-RPC 2.0 and lie in the I-JSON domain (`spec/identity.md` §1):
 *
 * - {@link FrameReader} splits a byte stream into frame bodies, and {@link parseMessage} reads a
 *   body strictly: a duplicate key, a lone surrogate, an integer that does not read exactly, a
 *   number out of range, `NaN`, nesting deeper than the engine writes or bytes that are not UTF-8
 *   are each a {@link ProtocolError}, worded as the Python host words them.
 * - {@link checkValue} refuses what `JSON.stringify` would quietly change: `undefined` (as a
 *   whole value or an array item), functions, symbols, bigints, `NaN` and the infinities, integers
 *   beyond ±2^53, lone surrogates, objects that are not plain (a `Map`, a `Date`, a class
 *   instance), a value that holds itself, and nesting deeper than the engine reads. An object
 *   member whose value is `undefined` is left out, as `JSON.stringify` and `toYaml` leave it out.
 *   {@link frame} checks every message before it is written, so nothing is ever coerced on the
 *   way out.
 */

export const PROTOCOL = "fx-node-protocol-v1";

/** Error codes (`spec/protocol.md` §7). */
export const PARSE_ERROR = -32700;
export const INVALID_REQUEST = -32600;
export const METHOD_NOT_FOUND = -32601;
export const INVALID_PARAMS = -32602;
export const NODE_FAILURE = -32000;
export const NODE_ERROR = -32001;
export const CANCELLED = -32002;
export const PROTOCOL_MISMATCH = -32003;
export const LOAD_FAILED = -32004;
export const BUILD_FAILED = -32005;
export const INTERNAL = -32099;

/** Every code of §7. An error a body lets through keeps its code only when it is one. */
export const ERROR_CODES: ReadonlySet<number> = new Set([
  PARSE_ERROR,
  INVALID_REQUEST,
  METHOD_NOT_FOUND,
  INVALID_PARAMS,
  NODE_FAILURE,
  NODE_ERROR,
  CANCELLED,
  PROTOCOL_MISMATCH,
  LOAD_FAILED,
  BUILD_FAILED,
  // capability_undeclared .. job_unsettled
  -32010, -32011, -32012, -32013, -32014, -32015, -32016, -32017,
  // agent_unfinished .. unknown_file
  -32020, -32021, -32022, -32023, -32024,
  INTERNAL,
]);

/** The deepest a value may sit in a message: inside at most this many arrays and objects, the
 * message's own object counted (the engine's reader refuses anything deeper). */
export const MAX_DEPTH = 512;

/** The longest header line read, its line feed included; a longer one breaks the transport. */
const MAX_HEADER_LINE = 8192;
const CONTENT_LENGTH = /^[ \t]*([0-9]{1,15})[ \t]*$/;
/** An integer literal longer than this cannot be canonical (at most 21 digits and a sign). */
const LONGEST_INTEGER = 22;
const LONE_SURROGATE = /\p{Cs}/u;
const LONE_SURROGATES = /\p{Cs}/gu;

/** A request id (`spec/protocol.md` §1). */
export type Id = number | string;

/** Whether a value is a request id: an integer or a string. */
export function validId(value: unknown): value is Id {
  return typeof value === "string" || (typeof value === "number" && Number.isInteger(value));
}

/** An error answer: `{code, message, data?}`. */
export interface ErrorBody {
  readonly code: number;
  readonly message: string;
  readonly data?: unknown;
}

/** A request answered with an error: its {@link body}. */
export class Refusal extends Error {
  readonly code: number;
  readonly data: unknown;

  constructor(code: number, message: string, data?: unknown) {
    super(message);
    this.name = "Refusal";
    this.code = code;
    this.data = data;
  }

  /** The error answer, which can always be sent: the message with its lone surrogates escaped
   * (`error` when it is empty), and `data` when there is any. */
  get body(): ErrorBody {
    const body: { code: number; message: string; data?: unknown } = {
      code: this.code,
      message: encodableText(this.message || "error"),
    };
    if (this.data !== undefined) {
      body.data = this.data;
    }
    return body;
  }
}

/** `String(value)`, or the empty string when that throws. */
export function safeString(value: unknown): string {
  try {
    return String(value);
  } catch {
    return "";
  }
}

/** A frame or a message that breaks the transport. */
export class ProtocolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ProtocolError";
  }
}

/**
 * Splits a byte stream into frame bodies. {@link push} returns the bodies a chunk completes, up to
 * where the stream breaks, if it does: {@link broken} then says why (a {@link ProtocolError}, which
 * ends the session), and nothing more is read. {@link end} throws when the stream ended inside a
 * frame.
 */
export class FrameReader {
  #broken: ProtocolError | null = null;
  /** The bytes read and not yet taken, in one buffer… */
  #buffer: Buffer = Buffer.alloc(0);
  /** …and the chunks after it, joined only once a body is whole, so a large body is copied once
   * however many chunks it comes in. */
  #chunks: Buffer[] = [];
  /** The bytes in {@link #chunks}. */
  #chunked = 0;
  /** The length the current header gives, once it gives one. */
  #found: number | null = null;
  /** The body's length once its header is read; null while a header is read. */
  #length: number | null = null;
  /** Whether any byte of the current frame was read. */
  #started = false;

  /** Why the stream broke, once it did. */
  get broken(): ProtocolError | null {
    return this.#broken;
  }

  push(chunk: Uint8Array): Buffer[] {
    const bodies: Buffer[] = [];
    if (this.#broken !== null || chunk.length === 0) {
      return bodies;
    }
    this.#chunks.push(Buffer.from(chunk));
    this.#chunked += chunk.length;
    try {
      this.#read(bodies);
    } catch (error) {
      if (!(error instanceof ProtocolError)) {
        throw error;
      }
      this.#broken = error;
      this.#buffer = Buffer.alloc(0);
      this.#chunks = [];
      this.#chunked = 0;
    }
    return bodies;
  }

  /** Joins the pending chunks onto the buffer. */
  #join(): void {
    if (this.#chunks.length > 0) {
      this.#buffer =
        this.#buffer.length === 0 && this.#chunks.length === 1
          ? (this.#chunks[0] as Buffer)
          : Buffer.concat([this.#buffer, ...this.#chunks], this.#buffer.length + this.#chunked);
      this.#chunks = [];
      this.#chunked = 0;
    }
  }

  #read(bodies: Buffer[]): void {
    for (;;) {
      if (this.#length !== null) {
        if (this.#buffer.length + this.#chunked < this.#length) {
          return;
        }
        this.#join();
        bodies.push(Buffer.from(this.#buffer.subarray(0, this.#length)));
        this.#buffer = this.#buffer.subarray(this.#length);
        this.#length = null;
        this.#found = null;
        this.#started = false;
        continue;
      }
      // A header: short lines, read from the joined bytes.
      this.#join();
      if (this.#buffer.length === 0) {
        return;
      }
      this.#started = true;
      const newline = this.#buffer.indexOf(0x0a);
      if (newline < 0) {
        if (this.#buffer.length >= MAX_HEADER_LINE) {
          throw new ProtocolError("a message header line is too long");
        }
        return;
      }
      if (newline + 1 > MAX_HEADER_LINE) {
        throw new ProtocolError("a message header line is too long");
      }
      let line = this.#buffer.subarray(0, newline);
      this.#buffer = this.#buffer.subarray(newline + 1);
      if (line.length > 0 && line[line.length - 1] === 0x0d) {
        line = line.subarray(0, line.length - 1);
      }
      if (line.length > 0) {
        this.#found = headerField(line, this.#found);
        continue;
      }
      if (this.#found === null) {
        throw new ProtocolError("a message header has no Content-Length");
      }
      this.#length = this.#found;
    }
  }

  /** The end of the stream: a {@link ProtocolError} when it ended inside a frame (and had not
   * broken before). */
  end(): void {
    if (this.#broken !== null) {
      return;
    }
    const pending = this.#buffer.length + this.#chunked;
    if (this.#length !== null) {
      throw new ProtocolError(
        `the stream ended after ${pending} of a message's ${this.#length} bytes`,
      );
    }
    if (this.#started || pending > 0) {
      throw new ProtocolError("the stream ended inside a message header");
    }
  }
}

/** The message's length once the header line `line` is read (`length` before it). */
function headerField(line: Buffer, length: number | null): number | null {
  const colon = line.indexOf(0x3a);
  if (colon < 0) {
    throw new ProtocolError(`a message header is not a field: ${bytesRepr(line.subarray(0, 80))}`);
  }
  const name = line
    .subarray(0, colon)
    .toString("latin1")
    .replace(/^[ \t\n\r\v\f]+|[ \t\n\r\v\f]+$/g, "");
  if (name.toLowerCase() !== "content-length") {
    return length; // other header fields are ignored
  }
  const value = line.subarray(colon + 1);
  const match = CONTENT_LENGTH.exec(value.toString("latin1"));
  if (match === null) {
    throw new ProtocolError(`Content-Length is not a byte count: ${bytesRepr(value.subarray(0, 80))}`);
  }
  const found = Number(match[1]);
  if (length !== null && found !== length) {
    throw new ProtocolError("a message header gives two different lengths");
  }
  return found;
}

/** A message as one whole frame: its header and its compact JSON body. A `TypeError` (nothing
 * written) when the message is not an I-JSON value ({@link checkValue}). */
export function frame(message: unknown): Buffer {
  const body = encodeMessage(message);
  return Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, "ascii"), body]);
}

/** A message's frame body: compact JSON, UTF-8. A `TypeError` when the message is not an I-JSON
 * value. */
export function encodeMessage(message: unknown): Buffer {
  checkValue(message);
  return Buffer.from(JSON.stringify(message), "utf8");
}

/** `text` with each lone surrogate written as its escape (`\udcff`), so a message holding it can
 * be sent: an error message built from a file name that is not UTF-8, say. */
export function encodableText(text: string): string {
  return text.replace(LONE_SURROGATES, (unit) => `\\u${hex(unit.charCodeAt(0), 4)}`);
}

/** A frame body as a JSON value; a {@link ProtocolError} for anything outside I-JSON. */
export function parseMessage(body: Uint8Array): unknown {
  const invalid = utf8Error(body);
  if (invalid !== null) {
    throw new ProtocolError(`the message is not UTF-8: ${invalid}`);
  }
  const text = new TextDecoder("utf-8", { ignoreBOM: true }).decode(body);
  return new JsonReader(text).document();
}

/** Where `bytes` stop being UTF-8, or null when they are. */
function utf8Error(bytes: Uint8Array): string | null {
  let index = 0;
  while (index < bytes.length) {
    const first = bytes[index] as number;
    if (first < 0x80) {
      index += 1;
      continue;
    }
    let needed: number;
    let lowest: number;
    let highest = 0xbf;
    if (first >= 0xc2 && first <= 0xdf) {
      needed = 1;
      lowest = 0x80;
    } else if (first >= 0xe0 && first <= 0xef) {
      needed = 2;
      lowest = first === 0xe0 ? 0xa0 : 0x80;
      highest = first === 0xed ? 0x9f : 0xbf;
    } else if (first >= 0xf0 && first <= 0xf4) {
      needed = 3;
      lowest = first === 0xf0 ? 0x90 : 0x80;
      highest = first === 0xf4 ? 0x8f : 0xbf;
    } else {
      return `byte 0x${hex(first, 2)} at position ${index} starts no character`;
    }
    for (let next = 1; next <= needed; next += 1) {
      const byte = bytes[index + next];
      const low = next === 1 ? lowest : 0x80;
      const high = next === 1 ? highest : 0xbf;
      if (byte === undefined) {
        return `the character at position ${index} is cut short`;
      }
      if (byte < low || byte > high) {
        return `byte 0x${hex(byte, 2)} at position ${index + next} continues no character`;
      }
    }
    index += needed + 1;
  }
  return null;
}

function hex(code: number, digits: number): string {
  return code.toString(16).padStart(digits, "0");
}

/** Whether an integer literal's digits are the canonical form of the number they read as. */
function canonicalInteger(text: string): boolean {
  if (text.length > LONGEST_INTEGER) {
    return false;
  }
  const value = BigInt(text);
  const magnitude = value < 0n ? -value : value;
  if (magnitude >= 10n ** 21n) {
    return false;
  }
  if (magnitude <= 2n ** 53n) {
    return true;
  }
  // The shortest digits that read back as the same double, written out in full.
  return String(Number(magnitude)) === magnitude.toString();
}

/** A strict JSON reader: JSON's grammar, with every I-JSON refusal of {@link parseMessage}. */
class JsonReader {
  #text: string;
  #at = 0;
  #depth = 0;
  #loneSurrogate = false;

  constructor(text: string) {
    this.#text = text;
  }

  document(): unknown {
    this.#space();
    const value = this.#value();
    this.#space();
    if (this.#at < this.#text.length) {
      this.#fail("extra data");
    }
    // As Python's reader: the strings are checked once the message has parsed.
    if (this.#loneSurrogate) {
      throw new ProtocolError("the message holds a lone surrogate");
    }
    return value;
  }

  #fail(what: string): never {
    throw new ProtocolError(`the message is not JSON: ${what} at character ${this.#at}`);
  }

  #space(): void {
    const text = this.#text;
    while (this.#at < text.length) {
      const code = text.charCodeAt(this.#at);
      if (code !== 0x20 && code !== 0x09 && code !== 0x0a && code !== 0x0d) {
        return;
      }
      this.#at += 1;
    }
  }

  #value(): unknown {
    const text = this.#text;
    const char = text[this.#at];
    switch (char) {
      case "{":
        return this.#object();
      case "[":
        return this.#array();
      case '"':
        return this.#string();
      case "t":
        return this.#word("true", true);
      case "f":
        return this.#word("false", false);
      case "n":
        return this.#word("null", null);
      case "N":
        return this.#constant("NaN");
      case "I":
        return this.#constant("Infinity");
      case undefined:
        return this.#fail("a value is missing");
      default:
        if (char === "-" && text.startsWith("-Infinity", this.#at)) {
          return this.#constant("-Infinity");
        }
        if (char === "-" || (char >= "0" && char <= "9")) {
          return this.#number();
        }
        return this.#fail("expecting a value");
    }
  }

  #word<T>(word: string, value: T): T {
    if (!this.#text.startsWith(word, this.#at)) {
      this.#fail("expecting a value");
    }
    this.#at += word.length;
    return value;
  }

  #constant(name: string): never {
    if (!this.#text.startsWith(name, this.#at)) {
      this.#fail("expecting a value");
    }
    throw new ProtocolError(`the message holds ${name}, which is not a JSON number`);
  }

  #enter(): void {
    this.#depth += 1;
    if (this.#depth > MAX_DEPTH + 1) {
      throw new ProtocolError("the message nests too deeply");
    }
  }

  #object(): Record<string, unknown> {
    this.#enter();
    this.#at += 1;
    const object: Record<string, unknown> = {};
    this.#space();
    if (this.#text[this.#at] === "}") {
      this.#at += 1;
      this.#depth -= 1;
      return object;
    }
    for (;;) {
      this.#space();
      if (this.#text[this.#at] !== '"') {
        this.#fail("expecting a property name in double quotes");
      }
      const key = this.#string();
      this.#space();
      if (this.#text[this.#at] !== ":") {
        this.#fail("expecting ':'");
      }
      this.#at += 1;
      this.#space();
      const value = this.#value();
      if (Object.hasOwn(object, key)) {
        throw new ProtocolError(`the message repeats the key ${pyRepr(key)} in one object`);
      }
      setOwn(object, key, value);
      this.#space();
      const next = this.#text[this.#at];
      this.#at += 1;
      if (next === "}") {
        this.#depth -= 1;
        return object;
      }
      if (next !== ",") {
        this.#at -= 1;
        this.#fail("expecting ',' or '}'");
      }
    }
  }

  #array(): unknown[] {
    this.#enter();
    this.#at += 1;
    const array: unknown[] = [];
    this.#space();
    if (this.#text[this.#at] === "]") {
      this.#at += 1;
      this.#depth -= 1;
      return array;
    }
    for (;;) {
      this.#space();
      array.push(this.#value());
      this.#space();
      const next = this.#text[this.#at];
      this.#at += 1;
      if (next === "]") {
        this.#depth -= 1;
        return array;
      }
      if (next !== ",") {
        this.#at -= 1;
        this.#fail("expecting ',' or ']'");
      }
    }
  }

  #string(): string {
    const text = this.#text;
    this.#at += 1;
    let out = "";
    let start = this.#at;
    for (;;) {
      if (this.#at >= text.length) {
        this.#fail("a string is not closed");
      }
      const code = text.charCodeAt(this.#at);
      if (code === 0x22) {
        out += text.slice(start, this.#at);
        this.#at += 1;
        break;
      }
      if (code < 0x20) {
        this.#fail("a control character in a string");
      }
      if (code !== 0x5c) {
        this.#at += 1;
        continue;
      }
      out += text.slice(start, this.#at);
      const escaped = text[this.#at + 1];
      this.#at += 2;
      switch (escaped) {
        case '"':
          out += '"';
          break;
        case "\\":
          out += "\\";
          break;
        case "/":
          out += "/";
          break;
        case "b":
          out += "\b";
          break;
        case "f":
          out += "\f";
          break;
        case "n":
          out += "\n";
          break;
        case "r":
          out += "\r";
          break;
        case "t":
          out += "\t";
          break;
        case "u": {
          const hex = text.slice(this.#at, this.#at + 4);
          if (!/^[0-9a-fA-F]{4}$/.test(hex)) {
            this.#fail("an invalid \\u escape");
          }
          out += String.fromCharCode(Number.parseInt(hex, 16));
          this.#at += 4;
          break;
        }
        default:
          this.#at -= 1;
          this.#fail("an invalid escape");
      }
      start = this.#at;
    }
    if (LONE_SURROGATE.test(out)) {
      this.#loneSurrogate = true;
    }
    return out;
  }

  #number(): number {
    const text = this.#text;
    const match = /-?(?:0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?/y;
    match.lastIndex = this.#at;
    const found = match.exec(text);
    if (found === null) {
      return this.#fail("expecting a value");
    }
    const literal = found[0];
    this.#at += literal.length;
    if (found[1] === undefined && found[2] === undefined) {
      if (!canonicalInteger(literal)) {
        throw new ProtocolError(`the integer ${literal.slice(0, 40)} cannot be read without rounding`);
      }
      // `-0` is the integer 0.
      return Number(literal) || 0;
    }
    const value = Number(literal);
    if (!Number.isFinite(value)) {
      throw new ProtocolError(`the number ${literal.slice(0, 40)} is too large`);
    }
    return value;
  }
}

/**
 * Throws a `TypeError` unless `value` is an I-JSON value the engine can read where it goes:
 * `null`, booleans, finite numbers (integers within ±2^53), strings without lone surrogates,
 * arrays and plain objects of those; an object member whose value is `undefined` is skipped (it is
 * never sent). `depth` is how many arrays and objects of its message hold it
 * (0 for a whole message; 1 for a request's params), and nothing in it may sit deeper than
 * {@link MAX_DEPTH}. A value that holds itself is refused. The message names where, as a JSON
 * pointer below `where`.
 */
export function checkValue(value: unknown, where = "", depth = 0): void {
  check(value, where, depth, new Set());
}

function check(value: unknown, where: string, depth: number, holding: Set<object>): void {
  switch (typeof value) {
    case "boolean":
      return;
    case "string":
      if (LONE_SURROGATE.test(value)) {
        throw new TypeError(`${at(where)}a string holds a lone surrogate`);
      }
      return;
    case "number":
      if (!Number.isFinite(value)) {
        throw new TypeError(`${at(where)}${String(value)} is not a JSON number`);
      }
      if (Number.isInteger(value) && Math.abs(value) > 2 ** 53) {
        throw new TypeError(`${at(where)}the integer ${shortInteger(value)} cannot be read exactly`);
      }
      return;
    case "undefined":
      throw new TypeError(`${at(where)}undefined is not a JSON value`);
    case "object":
      break;
    default:
      throw new TypeError(`${at(where)}${typeName(value)} is not a JSON value`);
  }
  if (value === null) {
    return;
  }
  const isArray = Array.isArray(value);
  if (!isArray && !isPlainObject(value)) {
    throw new TypeError(`${at(where)}${typeName(value)} is not a JSON value`);
  }
  if (holding.has(value)) {
    throw new TypeError(`${at(shortPointer(where))}the value holds itself`);
  }
  // An object member whose value is undefined is left out, as JSON.stringify leaves it out.
  const keys = isArray
    ? null
    : Object.keys(value).filter((key) => (value as Record<string, unknown>)[key] !== undefined);
  const size = keys === null ? (value as unknown[]).length : keys.length;
  if (size > 0 && depth >= MAX_DEPTH) {
    throw new TypeError(
      `${at(shortPointer(where))}the value nests deeper than the ${MAX_DEPTH} levels the engine reads`,
    );
  }
  holding.add(value);
  if (keys === null) {
    const items = value as unknown[];
    for (let index = 0; index < items.length; index += 1) {
      check(items[index], `${where}/${index}`, depth + 1, holding);
    }
  } else {
    for (const key of keys) {
      if (LONE_SURROGATE.test(key)) {
        throw new TypeError(`${at(where)}a key holds a lone surrogate`);
      }
      const item = (value as Record<string, unknown>)[key];
      check(item, `${where}/${pointerToken(key)}`, depth + 1, holding);
    }
  }
  holding.delete(value);
}

function at(where: string): string {
  return where ? `at ${where}: ` : "";
}

/** A JSON pointer cut to its first `keep` segments (`/value/0/0/0/…`). */
function shortPointer(where: string, keep = 4): string {
  const segments = where.split("/").slice(1);
  return segments.length <= keep ? where : `/${segments.slice(0, keep).join("/")}/…`;
}

/** Sets `object[key]` as an own, plain member, `__proto__` included. */
export function setOwn(object: object, key: string, value: unknown): void {
  Object.defineProperty(object, key, { value, writable: true, enumerable: true, configurable: true });
}

/** A key as a JSON pointer token (RFC 6901). */
export function pointerToken(key: string): string {
  return key.replaceAll("~", "~0").replaceAll("/", "~1");
}

function shortInteger(value: number): string {
  const digits = BigInt(Math.abs(value));
  if (Math.abs(value) < 1e40) {
    return BigInt(value).toString();
  }
  return `of ${digits.toString(2).length} bits`;
}

/** Whether `value` is an object JSON can hold as is: its prototype is `Object.prototype` or
 * none. */
export function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value) as unknown;
  return prototype === Object.prototype || prototype === null;
}

/** How messages name the type of a value: `undefined`, `null`, `a string`, `an array`, `a Map`,
 * `an InputFile`, `an object`. */
export function typeName(value: unknown): string {
  if (value === null) {
    return "null";
  }
  switch (typeof value) {
    case "undefined":
      return "undefined";
    case "string":
    case "number":
    case "boolean":
    case "bigint":
    case "symbol":
    case "function":
      return `a ${typeof value}`;
    default:
      break;
  }
  if (Array.isArray(value)) {
    return "an array";
  }
  if (isPlainObject(value)) {
    return "an object";
  }
  const object = value as Record<PropertyKey, unknown>;
  // Files of any copy of @grida/fx, by their brands.
  if (safely(() => object[Symbol.for("grida.fx.input_file")]) !== undefined) {
    return "an InputFile";
  }
  if (safely(() => object[Symbol.for("grida.fx.output")]) !== undefined) {
    return "an Output";
  }
  const constructor = safely(() => object.constructor);
  const name = typeof constructor === "function" ? constructor.name : "";
  if (!name) {
    return "an object";
  }
  return `${article(name)} ${name}`;
}

/** "a" or "an" before a name, by how it is said: an Item, a Uint8Array, a URL. */
function article(name: string): string {
  return /^[AEIO]/i.test(name) || /^Un(?!i)/.test(name) ? "an" : "a";
}

function safely<T>(read: () => T): T | undefined {
  try {
    return read();
  } catch {
    return undefined;
  }
}

/** A value as Python's `repr` writes it, the way the engine and the Python host quote names in
 * their messages: `'Bad'`, `['a', 'b']`. */
export function pyRepr(value: unknown): string {
  if (typeof value === "string") {
    return stringRepr(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(pyRepr).join(", ")}]`;
  }
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    return String(value);
  }
  if (value === null || value === undefined) {
    return String(value);
  }
  if (isPlainObject(value)) {
    return `{${Object.entries(value)
      .map(([key, item]) => `${stringRepr(key)}: ${pyRepr(item)}`)
      .join(", ")}}`;
  }
  return typeName(value);
}

function stringRepr(text: string): string {
  const quote = text.includes("'") && !text.includes('"') ? '"' : "'";
  let out = quote;
  for (const char of text) {
    const code = char.codePointAt(0) as number;
    if (char === "\\") {
      out += "\\\\";
    } else if (char === "\n") {
      out += "\\n";
    } else if (char === "\r") {
      out += "\\r";
    } else if (char === "\t") {
      out += "\\t";
    } else if (char === quote) {
      out += `\\${char}`;
    } else if (printable(code)) {
      out += char;
    } else if (code <= 0xff) {
      out += `\\x${code.toString(16).padStart(2, "0")}`;
    } else if (code <= 0xffff) {
      out += `\\u${code.toString(16).padStart(4, "0")}`;
    } else {
      out += `\\U${code.toString(16).padStart(8, "0")}`;
    }
  }
  return out + quote;
}

/** An approximation of Python's `str.isprintable` for one character. */
function printable(code: number): boolean {
  if (code < 0x7f) {
    return code >= 0x20;
  }
  if (code <= 0xa0 || code === 0xad) {
    return false;
  }
  return !(code >= 0xd800 && code <= 0xdfff) && !(code >= 0x2028 && code <= 0x2029);
}

/** Bytes as Python's `repr` writes them (`b'no colon here'`). */
function bytesRepr(bytes: Uint8Array): string {
  const text = Buffer.from(bytes).toString("latin1");
  const quote = text.includes("'") && !text.includes('"') ? '"' : "'";
  let out = `b${quote}`;
  for (const byte of bytes) {
    const char = String.fromCharCode(byte);
    if (char === "\\" || char === quote) {
      out += `\\${char}`;
    } else if (char === "\n") {
      out += "\\n";
    } else if (char === "\r") {
      out += "\\r";
    } else if (char === "\t") {
      out += "\\t";
    } else if (byte >= 0x20 && byte < 0x7f) {
      out += char;
    } else {
      out += `\\x${byte.toString(16).padStart(2, "0")}`;
    }
  }
  return out + quote;
}
