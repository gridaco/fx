/**
 * The literal specifiers a JavaScript module names (`spec/protocol.md` §5.1, "For TypeScript"),
 * found the way Python's rule finds imports: in the code, whether or not it runs.
 *
 * {@link literalSpecifiers} reads code (TypeScript after its types are stripped) with a small
 * tokenizer that skips comments, string literals, template literals (their `${…}` substitutions
 * are code and are read) and regular expression literals, telling a regular expression from a
 * division by the token before it, as JavaScript tools usually do: after an operand (a name, a
 * literal, `)`, `]`) a `/` divides, and after an operator, a punctuator or a keyword such as
 * `return` it starts a regular expression. A keyword that is a property (`stats.in`, `opts.default`)
 * is an operand, and a `)` that ends the head of an `if`, `while`, `for` or `with` is not: a
 * statement starts after it. A regular expression ends on its line, its body at most 65,536
 * characters long; a `/` that starts none is a division. The work is linear in the code. It then
 * finds:
 *
 * - `import … from "x"`, `import "x"` and `export … from "x"`;
 * - `import("x")` and `import("x", {…})`, with a literal first argument;
 * - `require("x")`, with a literal argument, when `require` is not a property (`a.require(…)`).
 *
 * A literal is a string literal or a template literal without substitutions. Anything computed
 * (`import(name)`, `` import(`./${name}.ts`) ``, `require("./" + name)`) is not a literal.
 */

/** A literal specifier: resolved as an `import` (ES module rules) or as a `require`. */
export interface Specifier {
  readonly kind: "import" | "require";
  readonly specifier: string;
}

type Token =
  | { readonly type: "name"; readonly value: string }
  | { readonly type: "punct"; readonly value: string }
  /** A string literal, or a template literal: `value` is null when it has substitutions. */
  | { readonly type: "string" | "template"; readonly value: string | null }
  | { readonly type: "regex" };

/** Words after which a `/` starts a regular expression. */
const BEFORE_EXPRESSION = new Set([
  "return",
  "typeof",
  "instanceof",
  "in",
  "of",
  "new",
  "delete",
  "void",
  "throw",
  "case",
  "do",
  "else",
  "yield",
  "await",
  "default",
]);

/** Punctuators after which a `/` is a division (a `)` that ends a statement's head aside). */
const AFTER_OPERAND = new Set([")", "]", "++", "--"]);

/** Words whose parenthesized head a statement follows: a `/` after its `)` starts a regular
 * expression. */
const HEADED = new Set(["if", "while", "for", "with"]);

/** How many tokens an import or export clause may hold outside its braces (`defer * as ns` is 4).
 * Its braces hold any number: a brace list ends at the first `{` or `}`, so no token is in two of
 * them, and the work stays linear in the code. */
const LONGEST_HEAD = 16;

/** The longest regular expression body read, in UTF-16 code units: a `/` whose body would be
 * longer is a division. */
const LONGEST_REGEX = 1 << 16;

/** The literal specifiers of `code` (module comment), in the order they appear. */
export function literalSpecifiers(code: string): Specifier[] {
  const tokens = tokenize(code);
  const found: Specifier[] = [];
  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index] as Token;
    if (token.type !== "name") {
      continue;
    }
    if (isProperty(tokens, index)) {
      continue;
    }
    const next = tokens[index + 1];
    if (token.value === "import") {
      if (isPunct(next, "(")) {
        const called = calledWith(tokens, index + 2);
        if (called !== null) {
          found.push({ kind: "import", specifier: called });
        }
      } else if (next?.type === "string" && next.value !== null) {
        found.push({ kind: "import", specifier: next.value });
      } else if (!isPunct(next, ".")) {
        const from = clauseSource(tokens, index + 1);
        if (from !== null) {
          found.push({ kind: "import", specifier: from });
        }
      }
    } else if (token.value === "export") {
      if (isPunct(next, "*") || isPunct(next, "{")) {
        const from = clauseSource(tokens, index + 1);
        if (from !== null) {
          found.push({ kind: "import", specifier: from });
        }
      }
    } else if (token.value === "require" && isPunct(next, "(")) {
      const called = calledWith(tokens, index + 2);
      if (called !== null) {
        found.push({ kind: "require", specifier: called });
      }
    }
  }
  return found;
}

function isPunct(token: Token | undefined, value: string): boolean {
  return token?.type === "punct" && token.value === value;
}

/** Whether the token at `index` follows a `.` or `?.`: a property's name, whatever word it is. */
function isProperty(tokens: readonly Token[], index: number): boolean {
  const before = tokens[index - 1];
  return isPunct(before, ".") || isPunct(before, "?.");
}

/** The literal at `index` when it is a call's first argument (`)` or `,` follows), or null. */
function calledWith(tokens: readonly Token[], index: number): string | null {
  const argument = tokens[index];
  if (argument === undefined || (argument.type !== "string" && argument.type !== "template")) {
    return null;
  }
  if (argument.value === null) {
    return null;
  }
  const after = tokens[index + 1];
  return isPunct(after, ")") || isPunct(after, ",") ? argument.value : null;
}

/**
 * The source of an import or export clause that starts at `index` (`x`, `{ a, "b" as c }`,
 * `* as ns`, `* as "ns"`, `x, { a }`, with words such as `defer` or `source`), or null when no
 * `from "…"` ends it.
 */
function clauseSource(tokens: readonly Token[], index: number): string | null {
  let braced = false;
  const last = Math.min(tokens.length, index + LONGEST_HEAD);
  for (let at = index; at < tokens.length && (braced || at < last); at += 1) {
    const token = tokens[at] as Token;
    if (braced) {
      if (isPunct(token, "}")) {
        const from = tokens[at + 1];
        const source = tokens[at + 2];
        if (from?.type === "name" && from.value === "from" && source?.type === "string") {
          return source.value;
        }
        return null;
      }
      if (!(token.type === "name" || token.type === "string" || isPunct(token, ","))) {
        return null;
      }
      continue;
    }
    if (isPunct(token, "{")) {
      braced = true;
    } else if (token.type === "name") {
      const source = tokens[at + 1];
      if (token.value === "from" && source?.type === "string") {
        return source.value;
      }
    } else if (token.type === "string") {
      // A namespace exported under a string name: `export * as "ns" from "x"`.
      const before = tokens[at - 1];
      if (!(before?.type === "name" && before.value === "as")) {
        return null;
      }
    } else if (!(isPunct(token, "*") || isPunct(token, ","))) {
      return null;
    }
  }
  return null;
}

// -- the tokenizer -------------------------------------------------------------------------------

function isLineTerminator(char: string): boolean {
  return char === "\n" || char === "\r" || char === " " || char === " ";
}

function isSpace(char: string): boolean {
  return (
    char === " " ||
    char === "\t" ||
    char === "\v" ||
    char === "\f" ||
    char === " " ||
    char === "﻿" ||
    isLineTerminator(char) ||
    (char >= " " && char <= " ") ||
    char === " " ||
    char === " " ||
    char === " " ||
    char === "　"
  );
}

function isNameChar(char: string): boolean {
  return (
    (char >= "a" && char <= "z") ||
    (char >= "A" && char <= "Z") ||
    (char >= "0" && char <= "9") ||
    char === "_" ||
    char === "$" ||
    char === "\\" ||
    (char > "\u007f" && !isSpace(char))
  );
}

function isDigit(char: string | undefined): boolean {
  return char !== undefined && char >= "0" && char <= "9";
}

function isHexDigit(char: string | undefined): boolean {
  return (
    char !== undefined && ((char >= "0" && char <= "9") || (char >= "a" && char <= "f") || (char >= "A" && char <= "F"))
  );
}

/** Whether a `/` after the last of `tokens` starts a regular expression; `headEnded` is whether
 * that token is a `)` that ends the head of an `if`, `while`, `for` or `with`. */
function regexMayStart(tokens: readonly Token[], headEnded: boolean): boolean {
  const previous = tokens[tokens.length - 1];
  if (previous === undefined) {
    return true;
  }
  switch (previous.type) {
    case "name":
      return BEFORE_EXPRESSION.has(previous.value) && !isProperty(tokens, tokens.length - 1);
    case "punct":
      return previous.value === ")" ? headEnded : !AFTER_OPERAND.has(previous.value);
    default:
      return false;
  }
}

/** Whether a `(` after the last of `tokens` opens a statement's head: it follows `if`, `while`,
 * `for`, `with` or `for await`, none of them a property. */
function opensHead(tokens: readonly Token[]): boolean {
  let at = tokens.length - 1;
  let word = tokens[at];
  if (word?.type === "name" && word.value === "await") {
    at -= 1;
    word = tokens[at];
    if (!(word?.type === "name" && word.value === "for")) {
      return false;
    }
  }
  return word?.type === "name" && HEADED.has(word.value) && !isProperty(tokens, at);
}

/** A string or template literal's escape at `code[at]` (after the backslash): what it stands for,
 * and where the literal goes on. */
function escaped(code: string, at: number): [string, number] {
  const char = code[at] ?? "";
  switch (char) {
    case "n":
      return ["\n", at + 1];
    case "t":
      return ["\t", at + 1];
    case "r":
      return ["\r", at + 1];
    case "b":
      return ["\b", at + 1];
    case "f":
      return ["\f", at + 1];
    case "v":
      return ["\v", at + 1];
    case "0":
      return ["\u0000", at + 1];
    case "x": {
      const hex = code.slice(at + 1, at + 3);
      return /^[0-9a-fA-F]{2}$/.test(hex) ? [String.fromCharCode(Number.parseInt(hex, 16)), at + 3] : ["x", at + 1];
    }
    case "u": {
      if (code[at + 1] === "{") {
        // Reads only the hex digits that follow, so the work stays linear in the code.
        let close = at + 2;
        while (isHexDigit(code[close])) {
          close += 1;
        }
        const closed = close > at + 2 && code[close] === "}";
        const value = closed ? Number.parseInt(code.slice(at + 2, close), 16) : Number.NaN;
        return value <= 0x10ffff ? [String.fromCodePoint(value), close + 1] : ["u", at + 1];
      }
      const hex = code.slice(at + 1, at + 5);
      return /^[0-9a-fA-F]{4}$/.test(hex) ? [String.fromCharCode(Number.parseInt(hex, 16)), at + 5] : ["u", at + 1];
    }
    case "\r":
      return ["", code[at + 1] === "\n" ? at + 2 : at + 1];
    case "\n":
    case " ":
    case " ":
      return ["", at + 1];
    default:
      return [char, at + 1];
  }
}

/**
 * Reads a template literal's text from `at` (after its backtick or the `}` that ends a
 * substitution) to its closing backtick, or to the `${` of its next substitution: the text,
 * where reading goes on, and whether a substitution follows.
 */
function templateText(code: string, at: number): { text: string; next: number; substitution: boolean } {
  let text = "";
  let index = at;
  while (index < code.length) {
    const char = code[index] as string;
    if (char === "`") {
      return { text, next: index + 1, substitution: false };
    }
    if (char === "$" && code[index + 1] === "{") {
      return { text, next: index + 2, substitution: true };
    }
    if (char === "\\") {
      const [value, next] = escaped(code, index + 1);
      text += value;
      index = next;
      continue;
    }
    text += char;
    index += 1;
  }
  return { text, next: index, substitution: false };
}

/** The tokens of `code` (module comment): comments and white space are dropped. */
function tokenize(code: string): Token[] {
  const tokens: Token[] = [];
  /** What each open `{` is: a block or object, or a template literal's substitution. */
  const braces: ("brace" | "substitution")[] = [];
  /** Whether each open `(` opens a statement's head ({@link opensHead}). */
  const parens: boolean[] = [];
  /** Whether the last `)` read ended a statement's head (read with a `)` as the last token). */
  let headEnded = false;
  const regexes = new RegexEnds(code);
  // A byte order mark, then a hashbang line.
  let index = code.startsWith("﻿") ? 1 : 0;
  if (code.startsWith("#!", index)) {
    index = lineEnd(code, index);
  }
  const length = code.length;
  while (index < length) {
    const char = code[index] as string;
    if (isSpace(char)) {
      index += 1;
      continue;
    }
    if (char === "/") {
      const next = code[index + 1];
      if (next === "/") {
        index = lineEnd(code, index);
        continue;
      }
      if (next === "*") {
        const close = code.indexOf("*/", index + 2);
        index = close === -1 ? length : close + 2;
        continue;
      }
      if (regexMayStart(tokens, headEnded)) {
        const end = regexes.end(index + 1);
        if (end !== null) {
          tokens.push({ type: "regex" });
          index = end;
          continue;
        }
      }
      tokens.push({ type: "punct", value: "/" });
      index += 1;
      continue;
    }
    if (char === '"' || char === "'") {
      let text = "";
      let at = index + 1;
      while (at < length) {
        const inside = code[at] as string;
        if (inside === char) {
          at += 1;
          break;
        }
        if (inside === "\n" || inside === "\r") {
          break; // unterminated: the line ends it
        }
        if (inside === "\\") {
          const [value, next] = escaped(code, at + 1);
          text += value;
          at = next;
          continue;
        }
        text += inside;
        at += 1;
      }
      tokens.push({ type: "string", value: text });
      index = at;
      continue;
    }
    if (char === "`") {
      const read = templateText(code, index + 1);
      index = read.next;
      if (read.substitution) {
        tokens.push({ type: "template", value: null });
        tokens.push({ type: "punct", value: "${" });
        braces.push("substitution");
      } else {
        tokens.push({ type: "template", value: read.text });
      }
      continue;
    }
    if (char === "(") {
      parens.push(opensHead(tokens));
      tokens.push({ type: "punct", value: "(" });
      index += 1;
      continue;
    }
    if (char === ")") {
      headEnded = parens.pop() ?? false;
      tokens.push({ type: "punct", value: ")" });
      index += 1;
      continue;
    }
    if (char === "{") {
      braces.push("brace");
      tokens.push({ type: "punct", value: "{" });
      index += 1;
      continue;
    }
    if (char === "}") {
      if (braces.pop() === "substitution") {
        const read = templateText(code, index + 1);
        index = read.next;
        if (read.substitution) {
          tokens.push({ type: "punct", value: "${" });
          braces.push("substitution");
        } else {
          tokens.push({ type: "template", value: null });
        }
        continue;
      }
      tokens.push({ type: "punct", value: "}" });
      index += 1;
      continue;
    }
    if (isDigit(char) || (char === "." && isDigit(code[index + 1]))) {
      let at = index + 1;
      while (at < length && (isNameChar(code[at] as string) || code[at] === ".")) {
        at += 1;
      }
      tokens.push({ type: "name", value: code.slice(index, at) });
      index = at;
      continue;
    }
    if (isNameChar(char) || (char === "#" && index + 1 < length && isNameChar(code[index + 1] as string))) {
      let at = index + 1;
      while (at < length && isNameChar(code[at] as string)) {
        at += 1;
      }
      tokens.push({ type: "name", value: code.slice(index, at) });
      index = at;
      continue;
    }
    const three = code.slice(index, index + 3);
    const two = code.slice(index, index + 2);
    if (three === "...") {
      tokens.push({ type: "punct", value: "..." });
      index += 3;
    } else if (two === "?." && !isDigit(code[index + 2])) {
      tokens.push({ type: "punct", value: "?." });
      index += 2;
    } else if (two === "++" || two === "--" || two === "=>") {
      tokens.push({ type: "punct", value: two });
      index += 2;
    } else {
      tokens.push({ type: "punct", value: char });
      index += 1;
    }
  }
  return tokens;
}

/** Where the line that holds `code[at]` ends (its line terminator, or the end). */
function lineEnd(code: string, at: number): number {
  let index = at;
  while (index < code.length && !isLineTerminator(code[index] as string)) {
    index += 1;
  }
  return index;
}

/**
 * Where regular expression literals end, in work linear in the code however many `/` turn out to
 * be divisions.
 *
 * A body is read from just after its `/` to the closing `/` outside a `[…]` class, a `\` escaping
 * the character after it; it fails at its line's end or after {@link LONGEST_REGEX} code units, and
 * the `/` is then a division. A later body that starts before the point where a failed one stopped
 * steps on the same characters (an escape pairs a `\` with the same character for both). Where the
 * failed one was outside a class, so is the later one, and the failed one met a `[` before any
 * `/`; where it was inside one, the later one is outside a class until its first `[` or `]`, and
 * after it both are in the same state. So the later body closes at its first `/` when no `[` or
 * `]` comes before it, and otherwise fails as the earlier one did: it is read only to its first
 * `/`, `[` or `]`. A body that starts before where the last such read stopped meets the same stop
 * and fails at once, so no character is read twice for one failure.
 */
class RegexEnds {
  readonly #code: string;
  /** Where the last body that failed stopped: its line's end, its limit or the code's end. */
  #failedAt = -1;
  /** Where the last read after it stopped at a `[` or `]` (or that read's limit). */
  #settled = -1;

  constructor(code: string) {
    this.#code = code;
  }

  /** Where the regular expression literal whose body starts at `at` ends (after its flags), or
   * null when it does not end (class comment): then the `/` was a division. */
  end(at: number): number | null {
    const code = this.#code;
    if (at < this.#failedAt) {
      if (at <= this.#settled) {
        return null;
      }
      const stop = bodyStop(code, at, this.#failedAt, false);
      if (stop < this.#failedAt && code[stop] === "/") {
        return flagsEnd(code, stop + 1);
      }
      this.#settled = stop;
      return null;
    }
    const limit = Math.min(code.length, at + LONGEST_REGEX);
    const stop = bodyStop(code, at, limit, true);
    if (stop < limit && code[stop] === "/") {
      return flagsEnd(code, stop + 1);
    }
    this.#failedAt = stop;
    this.#settled = -1;
    return null;
  }
}

/**
 * Reads a regular expression body from `at`, before `limit`: where it stops, which is the `/`
 * that closes it, a line terminator, or `limit`. With `classes`, a `/` inside a `[…]` class does
 * not close it; without, it also stops at its first `[` or `]` (see {@link RegexEnds}).
 */
function bodyStop(code: string, at: number, limit: number, classes: boolean): number {
  let index = at;
  let inClass = false;
  while (index < limit) {
    const char = code[index] as string;
    if (isLineTerminator(char)) {
      return index;
    }
    if (char === "\\") {
      // An escape holds any character but a line terminator.
      index += isLineTerminator(code[index + 1] ?? "") ? 1 : 2;
      continue;
    }
    if (char === "/" && !inClass) {
      return index;
    }
    if (char === "[" || char === "]") {
      if (!classes) {
        return index;
      }
      inClass = char === "[";
    }
    index += 1;
  }
  return limit;
}

/** Where a regular expression's flags that start at `at` end. */
function flagsEnd(code: string, at: number): number {
  let index = at;
  while (index < code.length && isNameChar(code[index] as string)) {
    index += 1;
  }
  return index;
}
