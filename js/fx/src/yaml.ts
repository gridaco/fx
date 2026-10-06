/**
 * Writing YAML that the engine reads: the strict subset of `spec/yaml.md`, written as the engine
 * writes it ("Writing YAML", `crates/grida-fx-core/src/yaml/write.rs`).
 *
 * Block style, two-space indentation, keys in the order the object holds them. A string is
 * written plain when, read back as a plain scalar, it is that same string; otherwise it is
 * double-quoted with JSON-style escapes. So `on`, `017`, `2026-10-05`, `1e3`, a digest like
 * `123e4567…`, `''`, and text with `: `, ` #` or a leading indicator are all quoted. Numbers are
 * written in their canonical form (ECMAScript's, as JCS writes them). The text ends with one
 * line feed.
 *
 * Only FX values are written (`spec/identity.md` §1): `null`, booleans, finite numbers, strings
 * without lone surrogates, arrays, and plain objects, nested at most 512 deep. An object member
 * whose value is `undefined` is left out, as JSON leaves it out; anything else is a `TypeError`
 * naming where it is. A key longer than 1024 characters cannot be written (YAML's limit for an
 * implicit key).
 */

/** How deep values may nest (`spec/yaml.md`, "Nesting"). */
const MAX_DEPTH = 512;
/** The longest key YAML writes as an implicit key. */
const MAX_KEY = 1024;

/** Writes `value` as a YAML document the engine reads back as the same value. */
export function toYaml(value: unknown): string {
  const plain = check(value, "the value", 0);
  if (isObject(plain) && Object.keys(plain).length > 0) {
    return writeMapping(plain, 0, false);
  }
  if (Array.isArray(plain) && plain.length > 0) {
    return writeSequence(plain, 0, false);
  }
  return `${scalarText(plain)}\n`;
}

/** Whether a string must be quoted to read back as itself. */
export function needsQuotes(text: string): boolean {
  // Anything a plain scalar would read as something else, or refuse.
  if (!resolvesToItself(text)) {
    return true;
  }
  // YAML 1.1's merge and value types: FX reads them as text, YAML 1.1 readers do not, and a plain
  // `<<` key is a merge key.
  if (text === "<<" || text === "=") {
    return true;
  }
  // A leading indicator starts something other than a plain scalar (or might, for `-`, `?` and
  // `:`); `---` and `...` at the start of a line are document markers.
  if ("-?:,[]{}#&*!|>'\"%@`".includes(text[0] ?? "") || text.startsWith("...")) {
    return true;
  }
  // Plain scalars lose leading and trailing white space; `: ` (or a final `:`) makes a key and
  // ` #` starts a comment.
  if (text.startsWith(" ") || text.endsWith(" ") || text.endsWith(":")) {
    return true;
  }
  if (text.includes(": ") || text.includes(" #")) {
    return true;
  }
  // Line breaks, tabs and other characters a plain scalar cannot hold as they are.
  for (const char of text) {
    if (needsEscape(char)) {
      return true;
    }
  }
  return false;
}

// ------------------------------------------------------------------------------------------------
// Plain-scalar resolution (spec/yaml.md "Values"), reduced to the question the writer asks.

/** Whether a plain scalar reads back as the string it is (not null, a boolean, a number, or
 * refused). */
function resolvesToItself(text: string): boolean {
  if (text === "" || text === "~" || text === "null" || text === "true" || text === "false") {
    return false;
  }
  // Every decimal form, integer or float, leading zeros included: a number, a refused number, or
  // an ambiguous form.
  if (isDecimal(text)) {
    return false;
  }
  return !isAmbiguous(text);
}

/** The ambiguous forms of yaml.md, beyond the decimal ones. */
function isAmbiguous(text: string): boolean {
  return (
    isWord(text) ||
    isRadix(text) ||
    (text.includes("_") && isNumberLike(text.replaceAll("_", ""))) ||
    isSexagesimal(text) ||
    isSpecial(text) ||
    isTimestamp(text)
  );
}

/** `yes|no|on|off|true|false|null` in any ASCII letter case. */
function isWord(text: string): boolean {
  const lower = asciiLower(text);
  return ["yes", "no", "on", "off", "true", "false", "null"].includes(lower);
}

/** `[-+]?(?:\.[0-9]+|[0-9]+\.[0-9]*|[0-9]+)(?:[eE][-+]?[0-9]+)?`, leading zeros included. */
function isDecimal(text: string): boolean {
  return /^[-+]?(?:\.[0-9]+|[0-9]+\.[0-9]*|[0-9]+)(?:[eE][-+]?[0-9]+)?$/.test(text);
}

/** `[-+]?0(?:[xX][0-9a-fA-F_]+|[oO][0-7_]+|[bB][01_]+)` */
function isRadix(text: string): boolean {
  return /^[-+]?0(?:[xX][0-9a-fA-F_]+|[oO][0-7_]+|[bB][01_]+)$/.test(text);
}

/** Any numeric reading of any YAML version: decimal, radix, sexagesimal. */
function isNumberLike(text: string): boolean {
  return isDecimal(text) || isRadix(text) || isSexagesimal(text);
}

/** `[-+]?[0-9]+(?::[0-9]+)+(?:\.[0-9]*)?` */
function isSexagesimal(text: string): boolean {
  return /^[-+]?[0-9]+(?::[0-9]+)+(?:\.[0-9]*)?$/.test(text);
}

/** `[-+]?\.inf` or `\.nan`, in any ASCII letter case. */
function isSpecial(text: string): boolean {
  const lower = asciiLower(text);
  return /^[-+]?\.inf$/.test(lower) || lower === ".nan";
}

/** YAML 1.1's timestamp type: `YYYY-MM-DD`, or a date with one- or two-digit month and day, `T`,
 * `t` or spaces and tabs, a time `H[H]:MM:SS[.fraction]`, then an optional zone (`Z` or
 * `±H[H][:MM]`) after optional spaces and tabs. */
function isTimestamp(text: string): boolean {
  return (
    /^[0-9]{4}-[0-9]{2}-[0-9]{2}$/.test(text) ||
    /^[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}(?:[Tt]|[ \t]+)[0-9]{1,2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]*)?(?:[ \t]*(?:Z|[-+][0-9]{1,2}(?::[0-9]{2})?))?$/.test(
      text,
    )
  );
}

function asciiLower(text: string): string {
  return text.replace(/[A-Z]/g, (letter) => letter.toLowerCase());
}

// ------------------------------------------------------------------------------------------------
// Writing

/** Characters written as an escape inside double quotes: C0 and C1 controls (tab and line feed
 * included), DEL, U+0085, U+2028, U+2029, the byte-order mark, and U+FFFE and U+FFFF. */
function needsEscape(char: string): boolean {
  const code = char.codePointAt(0) ?? 0;
  return (
    code <= 0x1f ||
    (code >= 0x7f && code <= 0x9f) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0xfeff ||
    code === 0xfffe ||
    code === 0xffff
  );
}

type Plain = null | boolean | number | string | Plain[] | { [key: string]: Plain };

function writeMapping(map: { [key: string]: Plain }, indent: number, inline: boolean): string {
  let out = "";
  Object.entries(map).forEach(([key, item], index) => {
    if (index > 0 || !inline) {
      out += " ".repeat(indent);
    }
    out += `${keyText(key)}:`;
    if (isObject(item) && Object.keys(item).length > 0) {
      out += `\n${writeMapping(item, indent + 2, false)}`;
    } else if (Array.isArray(item) && item.length > 0) {
      out += `\n${writeSequence(item, indent + 2, false)}`;
    } else {
      out += ` ${scalarText(item)}\n`;
    }
  });
  return out;
}

function writeSequence(items: Plain[], indent: number, inline: boolean): string {
  let out = "";
  items.forEach((item, index) => {
    if (index > 0 || !inline) {
      out += " ".repeat(indent);
    }
    out += "-";
    if (isObject(item) && Object.keys(item).length > 0) {
      out += ` ${writeMapping(item, indent + 2, true)}`;
    } else if (Array.isArray(item) && item.length > 0) {
      out += ` ${writeSequence(item, indent + 2, true)}`;
    } else {
      out += ` ${scalarText(item)}\n`;
    }
  });
  return out;
}

function keyText(key: string): string {
  return needsQuotes(key) ? quoted(key) : key;
}

/** A scalar or an empty collection, as it is written after `key: ` or `- `. */
function scalarText(value: Plain): string {
  if (value === null) {
    return "null";
  }
  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }
  if (typeof value === "number") {
    // ECMAScript's Number.prototype.toString is JCS's number form; -0 is 0.
    return Object.is(value, -0) ? "0" : String(value);
  }
  if (typeof value === "string") {
    return needsQuotes(value) ? quoted(value) : value;
  }
  return Array.isArray(value) ? "[]" : "{}";
}

/** A double-quoted scalar with JSON's escapes (YAML's double-quoted escapes include them all). */
function quoted(text: string): string {
  let out = '"';
  for (const char of text) {
    switch (char) {
      case '"':
        out += '\\"';
        break;
      case "\\":
        out += "\\\\";
        break;
      case "\b":
        out += "\\b";
        break;
      case "\t":
        out += "\\t";
        break;
      case "\n":
        out += "\\n";
        break;
      case "\f":
        out += "\\f";
        break;
      case "\r":
        out += "\\r";
        break;
      default:
        out += needsEscape(char)
          ? `\\u${(char.codePointAt(0) ?? 0).toString(16).padStart(4, "0")}`
          : char;
    }
  }
  return `${out}"`;
}

// ------------------------------------------------------------------------------------------------
// Checking

function isObject(value: unknown): value is { [key: string]: Plain } {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;

/** `value` as a plain FX value, or a `TypeError` naming `where` it is not one. */
function check(value: unknown, where: string, depth: number): Plain {
  if (value === null || typeof value === "boolean") {
    return value;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      throw new TypeError(`${where} is ${value}, which is not a number FX can hold`);
    }
    return value;
  }
  if (typeof value === "string") {
    if (LONE_SURROGATE.test(value)) {
      throw new TypeError(`${where} holds a lone surrogate, which is not text FX can hold`);
    }
    return value;
  }
  if (depth >= MAX_DEPTH && (Array.isArray(value) || isObject(value))) {
    throw new TypeError(`${where} nests deeper than ${MAX_DEPTH} collections`);
  }
  if (Array.isArray(value)) {
    return value.map((item, index) => {
      if (item === undefined) {
        throw new TypeError(`${where}[${index}] is undefined`);
      }
      return check(item, `${where}[${index}]`, depth + 1);
    });
  }
  if (isObject(value)) {
    const out: { [key: string]: Plain } = {};
    for (const [key, item] of Object.entries(value)) {
      if (item === undefined) {
        continue;
      }
      const at = `${where}[${JSON.stringify(key)}]`;
      if (LONE_SURROGATE.test(key)) {
        throw new TypeError(`${at}: the key holds a lone surrogate`);
      }
      if ([...key].length > MAX_KEY) {
        throw new TypeError(`${at}: a key longer than ${MAX_KEY} characters cannot be written`);
      }
      out[key] = check(item, at, depth + 1);
    }
    return out;
  }
  const kind = typeof value === "object" ? (value.constructor?.name ?? "object") : typeof value;
  throw new TypeError(`${where} is a ${kind}, which is not a value FX can hold`);
}
