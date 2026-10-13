/**
 * Source closures for the TypeScript node host (`spec/protocol.md` §5.1, "For TypeScript").
 *
 * A closure has two kinds of edge (importer → imported file):
 *
 * - Literal edges. Every source module names files with literal specifiers ({@link
 *   literalSpecifiers}: `import … from "x"`, `import "x"`, `export … from "x"`, `import("x")`,
 *   `require("x")`), and these count whether or not the code runs, as Python's rule counts every
 *   import in a file. A TypeScript module is read after Node's own type stripping, so a type-only
 *   import (`import type`, `typeof import("x")`) names nothing. A literal resolves as Node would
 *   resolve it from the module: a relative `import` to exactly the file it names (an ES module
 *   import needs the extension), a `#` subpath import or a package name by Node's lookup with the
 *   conditions of an import, and a `require` as `createRequire(<module>).resolve` resolves it. A
 *   literal that resolves to no file, or to a file that is not source, names nothing. A file a
 *   literal names is read the same way whether or not Node ever loaded it (each file once per
 *   session), so a closure through files that never ran is complete and does not depend on what
 *   ran.
 * - Recorded edges, every resolution Node makes, so Node's own rules decide (`#` subpath imports,
 *   `package.json` `exports`, symbolic links resolved to real paths): `module.registerHooks` sees
 *   each `import`, and each `require` (a CommonJS module's own, and `createRequire`'s) is recorded
 *   by a wrapper around `Module.prototype.require`, since Node answers a repeated `require` of one
 *   specifier from one folder from its own cache without calling the hooks. A recorded edge counts
 *   only once its file reached Node's `load` hook, so `import.meta.resolve` alone adds nothing, and
 *   `require.resolve` records no edge. Only recorded edges know computed specifiers
 *   (`` import(`./plugins/${name}.ts`) ``).
 *
 * The closure of a module is the module and every source file reachable from it along both kinds
 * of edge:
 *
 * - a project file: under the project root, with no `node_modules` folder below the root;
 * - a file of a declared source package: under the real folder of `node_modules/<name>` found from
 *   the project root up (as Node finds a package), with no `node_modules` folder below it.
 *
 * Anything else (a dependency, a builtin, a data URL) is not source and is not followed, so a
 * project file only a dependency loads (a configuration loader's `import(url)`) is in no closure.
 * Labels are project-relative POSIX paths, or `<package name>/<path in the package>` for a source
 * package (the nearer base wins), and a closure is sorted by label.
 *
 * Whose a recorded edge is, so a closure never depends on what the session loaded before. A source
 * module Node loads as an ES module is pending from its `load` until its top level ends: the host
 * appends one call on a new last line (so every line and column stays) that tells it so, under a
 * global name chosen when the hooks are installed, which no user binding shadows. A CommonJS
 * module's top level runs until Node marks it `loaded` (one no longer in `require.cache` has
 * finished). An edge whose importer's top level has not ended is the importer's own: its static
 * imports (resolved before its top level runs) and what its top level imports or requires itself.
 * A `require` counts for the module whose `require` it is: a CommonJS module's own, or, through
 * `createRequire`, the ES module whose URL made it; a `createRequire` from anything else (a
 * folder, a `package.json`, a URL with a query) counts for the source module that calls it. Any
 * other edge is made at call time, by a function of a module that finished: it belongs to each
 * unfinished source module with a frame on the stack, awaiting frames included (a shared
 * registry's lazy `import()` that another module's top level calls counts for that module, never
 * for every importer of the registry); to every pending module when the stack shows none (a
 * callback with no awaiting frame behind it); and, when nothing is pending, to the run whose async
 * context made it ({@link Closures.openRun}), while that run is open: the host checks it after the
 * body and then drops it. An edge from work a finished run left behind (a timer) counts for no run.
 *
 * Known limits. A module whose top level outlives the host's import of it (an unawaited `import()`
 * whose module awaits) is taken as finished when that import ends, so what it loads later is a
 * call-time edge, and the run's when a run's async context made it. A callback with no awaiting frame, run while several
 * modules are pending, counts for each of them. `import.meta.resolve` of a file that some other
 * module does load counts once that file is loaded, because the `resolve` hook cannot tell it from
 * an import. A computed specifier in a loader that memoizes what it loaded (a registry's cache) is
 * made once, so it counts for the first module whose top level called it: write such specifiers as
 * literals. In these cases a closure can depend on what was described first. And the hooks see
 * only the host's own thread: what a body loads in a worker thread (`new Worker(…)`) or a child
 * process is in no closure, and the run-time guard never sees it; name such files in `resources`.
 *
 * The run-time guard ({@link Closures.closeRun}) fails a run for a source file outside the
 * module's closure from its first load that this run loaded first, or that only this run's own
 * edges reach (a body's `import()` of a file some other module had loaded). A file the module's
 * own edges reach, and this run did not load, never fails a run, so what one run or another
 * module loaded never fails a later, innocent run. A literal `import()` a body runs never fails it:
 * its file is in the closure. The file the guard names is one Node loaded: a file only a literal
 * names, never loaded, is reached through one that was.
 *
 * A file counts only when every part of its path below its base is spelled exactly as its
 * directory entry: Node on a file system that ignores case loads `lib/PNG.ts` for `lib/png.ts`,
 * and {@link Closures.misspelling} finds it so `describe` can refuse the module. A literal that
 * names a file so misspelled names nothing.
 *
 * Two more rules live in the hooks: a bare `@grida/fx` (or `@grida/fx/<sub>`) the project cannot
 * resolve resolves to the host's own package, and a module the host loads as a node module, a
 * builder or a stand-in file must be an ES module ({@link NotAnEsModule}), whatever loaded it
 * first.
 */

import { AsyncLocalStorage } from "node:async_hooks";
import { randomBytes } from "node:crypto";
import { readdirSync, readFileSync, realpathSync, statSync } from "node:fs";
import * as nodeModule from "node:module";
import { dirname, extname, isAbsolute, join, relative, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { literalSpecifiers } from "./specifiers.js";

/** One file of a closure: its label and its absolute real path. */
export interface ClosureEntry {
  readonly label: string;
  readonly path: string;
}

/** A misspelled file of a closure: its label as imported, and as its directory entries spell
 * it; `normalization` when the two differ only in Unicode normalization. */
export interface Misspelling {
  readonly imported: string;
  readonly onDisk: string;
  readonly normalization: boolean;
}

/** A module Node loads as CommonJS where the host needs an ES module. */
export class NotAnEsModule extends Error {
  constructor(url: string) {
    super(`${url} is not an ES module`);
    this.name = "NotAnEsModule";
  }
}

/** Where source files are: the project root, or a source package's folder with its label
 * prefix. */
interface Base {
  readonly folder: string;
  readonly prefix: string;
}

/** Edges by importer. */
type Edges = Map<string, Set<string>>;

/**
 * One run's scope ({@link Closures.openRun}): the call-time edges no top level claims, and the
 * source files (real paths) first loaded, by the work that runs in it (the body and what it
 * started, and the host's work for the run) while it is open.
 */
export class RunScope {
  readonly edges: Map<string, Set<string>> = new Map();
  readonly loaded: Set<string> = new Set();
  open = true;
}

/** What the host reads of Node's CommonJS loader (`node:module`'s `Module`). */
interface CommonJsModule {
  readonly filename?: unknown;
  readonly loaded?: unknown;
}
interface CommonJsLoader {
  readonly prototype: { require(this: CommonJsModule, id: string): unknown };
  readonly _cache: Record<string, CommonJsModule | undefined>;
  _resolveFilename(
    request: string,
    parent: CommonJsModule,
    isMain?: boolean,
    options?: { readonly conditions?: ReadonlySet<string> },
  ): string;
  _nodeModulePaths(from: string): string[];
  new (id: string): CommonJsModule & { filename: string; paths: string[] };
}

const OWN_PACKAGE = "@grida/fx";
const COMMONJS = new Set(["commonjs", "commonjs-typescript"]);
const ES_MODULE = new Set(["module", "module-typescript"]);
const LOADER = nodeModule.Module as unknown as CommonJsLoader;
/** This module's own URL, whose frames are never a caller's. */
const OWN_URL = import.meta.url;
/** The files whose literal specifiers count, and those of them read after type stripping. */
const SCANNED = new Set([".js", ".mjs", ".cjs", ".ts", ".mts", ".cts"]);
const TYPESCRIPT = new Set([".ts", ".mts", ".cts"]);
/** The conditions of an import until the `resolve` hook has seen one (Node 24's defaults). */
const IMPORT_CONDITIONS = ["node", "import", "module-sync", "node-addons"];
/** How many frames {@link callerFile} reads. */
const CALLER_DEPTH = 16;

/** Whether a resolution with `conditions` is a `require`'s (recorded by the wrapper), not an
 * import's: a user condition named `require` puts it in every import's conditions too. */
function requiring(conditions: readonly string[]): boolean {
  return conditions.includes("require") && !conditions.includes("import");
}

/** The module graph Node resolved, read as closures (module comment). */
export class Closures {
  /** Each module's own edges: made before its top level ended. */
  readonly #intrinsic: Edges = new Map();
  /** Call-time edges, by the module whose top level made the call. */
  readonly #attributed: Edges = new Map();
  /** The scope of the run whose async context is current, if any. */
  readonly #runs = new AsyncLocalStorage<RunScope>();
  /** Each source file's literal targets (file URLs), by real path: read once per session. */
  readonly #literal = new Map<string, readonly string[]>();
  /** The conditions Node resolves an import with, once the `resolve` hook has seen one. */
  #importConditions: readonly string[] | null = null;
  /** {@link entry}'s and {@link #baseOf}'s answers, which hold until {@link configure}. */
  readonly #entries = new Map<string, ClosureEntry | null>();
  readonly #bases = new Map<string, Base | null>();
  /** Every URL that reached the `load` hook, with the format Node loads it as. */
  readonly #loaded = new Map<string, string>();
  /** Every source file (real path) that reached the `load` hook. */
  readonly #loadedFiles = new Set<string>();
  /** Source ES modules that reached the `load` hook and whose top level has not ended. */
  readonly #pending = new Set<string>();
  /** Source ES modules whose top level ended (or failed). */
  readonly #finished = new Set<string>();
  /** Modules the host imports now that must load as ES modules. */
  readonly #expected = new Set<string>();
  /** The global function a pending module calls as its top level ends (module comment). */
  #marker = "";
  #root: Base | null = null;
  #packages: Base[] = [];

  /**
   * Starts recording. `ownUrl` is a module of this package: a bare `@grida/fx` the importer cannot
   * resolve is resolved from there instead (Node's self-reference), so a project run through a
   * launcher's `@grida/fx` loads without a local install; when that fails too, the importer's own
   * error stands.
   */
  install(ownUrl: string): void {
    const ownRequire = nodeModule.createRequire(ownUrl);
    this.#marker = `__grida_fx_evaluated_${randomBytes(8).toString("hex")}`;
    Object.defineProperty(globalThis, this.#marker, {
      value: (url: unknown) => {
        if (typeof url === "string") {
          this.#pending.delete(url);
          this.#finished.add(url);
        }
      },
      enumerable: false,
      writable: false,
      configurable: false,
    });
    // `createRequire`'s require and each CommonJS module's go through here, `this` the requiring
    // module, cached or not.
    const closures = this;
    const require = LOADER.prototype.require;
    LOADER.prototype.require = function (this: CommonJsModule, id: string): unknown {
      try {
        return require.call(this, id);
      } finally {
        try {
          closures.#required(this, id);
        } catch {
          // recording never changes what require does
        }
      }
    };
    nodeModule.registerHooks({
      resolve: (specifier, context, nextResolve) => {
        let resolved;
        try {
          resolved = nextResolve(specifier, context);
        } catch (error) {
          const code = (error as { code?: unknown }).code;
          const missing = code === "ERR_MODULE_NOT_FOUND" || code === "MODULE_NOT_FOUND";
          if (!ownPackage(specifier) || !missing) {
            throw error;
          }
          try {
            // `require` resolves from the requiring module whatever `parentURL` says.
            resolved = requiring(context.conditions)
              ? { url: pathToFileURL(ownRequire.resolve(specifier)).href, shortCircuit: true }
              : nextResolve(specifier, { ...context, parentURL: ownUrl });
          } catch {
            throw error;
          }
        }
        // A `require` is recorded by the wrapper above, which also sees the ones Node answers from
        // its cache; `require.resolve` loads nothing.
        if (!requiring(context.conditions)) {
          this.#importConditions ??= [...context.conditions];
          if (context.parentURL !== undefined) {
            this.#record(context.parentURL, resolved.url);
          }
        }
        return resolved;
      },
      load: (url, context, nextLoad) => {
        const loaded = nextLoad(url, context);
        const format = String(loaded.format);
        this.#loaded.set(url, format);
        if (this.#expected.has(url) && COMMONJS.has(format)) {
          throw new NotAnEsModule(url);
        }
        const entry = this.entry(url);
        if (entry === null) {
          return loaded;
        }
        if (!this.#loadedFiles.has(entry.path)) {
          this.#loadedFiles.add(entry.path);
          this.#openScope()?.loaded.add(entry.path);
        }
        const source = loaded.source;
        if (source !== undefined && source !== null && ES_MODULE.has(format)) {
          this.#finished.delete(url);
          this.#pending.add(url);
          return { ...loaded, source: marked(source, this.#marker) };
        }
        return loaded;
      },
    });
  }

  /** Records what the CommonJS `require(id)` of `module` loaded, as an edge from the module that
   * requires (module comment): `module` when Node loaded it (a CommonJS module, or the ES module
   * a `createRequire` was made for), else the calling source module. */
  #required(module: CommonJsModule | undefined, id: string): void {
    const filename = module?.filename;
    if (module === undefined || typeof filename !== "string" || !isAbsolute(filename)) {
      return;
    }
    if (nodeModule.isBuiltin(id)) {
      return;
    }
    let importer: string;
    if (LOADER._cache[filename] === module) {
      if (this.#baseOf(filename) === null) {
        return; // a dependency's
      }
      importer = pathToFileURL(filename).href;
    } else {
      importer = pathToFileURL(filename).href;
      if (!ES_MODULE.has(this.#loaded.get(importer) ?? "")) {
        // A `createRequire` from a folder, a `package.json` or a URL with a query.
        const caller = callerFile();
        if (caller === null) {
          return;
        }
        importer = caller.startsWith("file:") ? caller : pathToFileURL(caller).href;
      }
      if (this.entry(importer) === null) {
        return;
      }
    }
    let target: string;
    try {
      target = LOADER._resolveFilename(id, module);
    } catch {
      return;
    }
    if (isAbsolute(target)) {
      this.#record(importer, pathToFileURL(target).href);
    }
  }

  /** Records the edge `parent` → `target` as its importer's own, or as a call-time edge (module
   * comment). Edges are followed only from source files, so a dependency's are not kept: what only
   * a dependency loads is in no closure, whenever it loads it. */
  #record(parent: string, target: string): void {
    if (this.entry(parent) === null) {
      return;
    }
    if (!this.#ended(parent)) {
      add(this.#intrinsic, parent, target);
      return;
    }
    if (this.entry(target) === null) {
      return;
    }
    const owners = this.#owners();
    if (owners.length > 0) {
      for (const owner of owners) {
        add(this.#attributed, owner, target);
      }
      return;
    }
    const scope = this.#openScope();
    if (scope !== null) {
      add(scope.edges, parent, target);
    }
  }

  /** The scope of the run whose async context is current, while that run is open; else null
   * (work a finished run left behind, or no run). */
  #openScope(): RunScope | null {
    const scope = this.#runs.getStore();
    return scope !== undefined && scope.open ? scope : null;
  }

  /** Whether the top level of the module at `url` has ended: an ES module's once its marker ran (or
   * the import or run it started in ended), a CommonJS module's once Node marks it loaded or it
   * left `require.cache` (it deleted itself, or its top level threw). */
  #ended(url: string): boolean {
    if (this.#finished.has(url)) {
      return true;
    }
    if (!COMMONJS.has(this.#loaded.get(url) ?? "")) {
      return false;
    }
    const path = filePath(url);
    if (path === null) {
      return false;
    }
    const cached = LOADER._cache[path];
    return cached === undefined || cached.loaded === true;
  }

  /** The modules a call-time edge made now belongs to (module comment): the unfinished source
   * modules with a frame on the stack, else every pending module. */
  #owners(): string[] {
    const owners: string[] = [];
    for (const file of stackFiles(Number.POSITIVE_INFINITY) ?? []) {
      if (file.startsWith("file:")) {
        if (this.#pending.has(file)) {
          owners.push(file);
        }
      } else if (isAbsolute(file) && this.#baseOf(file) !== null && LOADER._cache[file]?.loaded === false) {
        owners.push(pathToFileURL(file).href);
      }
    }
    return owners.length > 0 ? owners : [...this.#pending];
  }

  /** The project root (real path) and the declared source packages, at `initialize`. A name that
   * resolves to no package adds no base. */
  configure(root: string, sources: readonly string[]): void {
    this.#entries.clear();
    this.#bases.clear();
    this.#literal.clear();
    this.#root = { folder: root, prefix: "" };
    this.#packages = [];
    for (const name of sources) {
      const folder = packageFolder(root, name);
      if (folder !== null) {
        this.#packages.push({ folder, prefix: name });
      }
    }
  }

  /** Imports the module at `url`, which must be an ES module: one Node loads as CommonJS fails with
   * {@link NotAnEsModule}, before any of its code runs when this is its first load, and after it
   * when another module loaded it first. */
  async importModule(url: string): Promise<Record<string, unknown>> {
    this.#expected.add(url);
    try {
      const namespace = (await import(url)) as Record<string, unknown>;
      if (COMMONJS.has(this.#loaded.get(url) ?? "")) {
        throw new NotAnEsModule(url);
      }
      return namespace;
    } finally {
      this.#expected.delete(url);
      this.settle();
    }
  }

  /** Every pending module ends: what an import left that failed (or never ended) is done, so later
   * edges are not charged to it. */
  settle(): void {
    for (const url of this.#pending) {
      this.#finished.add(url);
    }
    this.#pending.clear();
  }

  /** A new run's scope: what work run {@link within} it makes while it is open (call-time edges no
   * top level claims, files first loaded) is kept for {@link closeRun}. */
  openRun(): RunScope {
    return new RunScope();
  }

  /** Runs `work` in the async context of the run `scope`: everything it starts (promises, timers,
   * callbacks) records into that scope while it is open. */
  within<T>(scope: RunScope, work: () => T): T {
    return this.#runs.run(scope, work);
  }

  /**
   * Closes the run `scope` and answers what the run loaded outside the closure of the module at
   * `url`: the files that are not in `baseline` (the closure's real paths when the module first
   * loaded), that the module reaches through its edges or the run's own, and that this run loaded
   * first or that only the run's own edges reach: those Node loaded, sorted by label, then those
   * only a literal names, which Node never loaded, sorted by label. Such a file is reached through
   * one Node did load, which is outside the closure too and is the one to name. The run's edges
   * are then dropped, and what the run left running records nothing more, so what one run loaded
   * never fails another run.
   */
  closeRun(scope: RunScope, url: string, baseline: ReadonlySet<string>): ClosureEntry[] {
    scope.open = false;
    this.settle();
    if (scope.edges.size === 0 && scope.loaded.size === 0) {
      return []; // most runs: the closure is the module's own, and the run loaded nothing new
    }
    // Without edges of the run's own, the walk is the module's own closure, and nothing in it
    // is reached only through the run's edges.
    const own =
      scope.edges.size === 0 ? null : new Set(this.#walk(url, null).map((entry) => entry.path));
    const outside = this.#walk(url, scope.edges).filter(
      (entry) =>
        !baseline.has(entry.path) &&
        (scope.loaded.has(entry.path) || (own !== null && !own.has(entry.path))),
    );
    const neverLoaded = (entry: ClosureEntry) => (this.#loadedFiles.has(entry.path) ? 0 : 1);
    // Sorted by label already: a stable sort keeps that order within each part.
    return outside.sort((a, b) => neverLoaded(a) - neverLoaded(b));
  }

  /** The closure of the module at `url`, sorted by label. */
  closure(url: string): ClosureEntry[] {
    return this.#walk(url, null);
  }

  /** The source files reachable from `url` along literal edges, recorded edges and `run`'s
   * (module comment), sorted by label. */
  #walk(url: string, run: Edges | null): ClosureEntry[] {
    const counted = new Set<string>();
    const found = new Map<string, ClosureEntry>();
    // Each URL to visit, and whether it counts though Node never loaded it (a literal's file).
    const frontier: [string, boolean][] = [[url, true]];
    while (frontier.length > 0) {
      const [current, literal] = frontier.pop() as [string, boolean];
      if (counted.has(current)) {
        continue;
      }
      const entry = this.entry(current);
      if (entry === null || (!literal && !this.#loaded.has(current))) {
        continue;
      }
      counted.add(current);
      found.set(entry.path, entry);
      for (const edges of [this.#intrinsic, this.#attributed, run]) {
        for (const imported of edges?.get(current) ?? []) {
          frontier.push([imported, false]);
        }
      }
      for (const target of this.#literalTargets(entry.path)) {
        frontier.push([target, true]);
      }
    }
    return [...found.values()].sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));
  }

  /** The source files the file at `path` names with literal specifiers (module comment), as file
   * URLs; the file is read once per session. */
  #literalTargets(path: string): readonly string[] {
    let targets = this.#literal.get(path);
    if (targets === undefined) {
      targets = this.#readLiterals(path);
      this.#literal.set(path, targets);
    }
    return targets;
  }

  /** Reads the literal specifiers of the source file at `path` and resolves each (module
   * comment): the source files they name. */
  #readLiterals(path: string): string[] {
    const extension = extname(path);
    if (!SCANNED.has(extension)) {
      return [];
    }
    let code: string;
    try {
      code = readFileSync(path, "utf8");
    } catch {
      return [];
    }
    if (TYPESCRIPT.has(extension)) {
      const stripped = stripTypes(code);
      if (stripped === null) {
        return [];
      }
      code = stripped;
    }
    const targets = new Set<string>();
    const listed = new Map<string, readonly string[]>();
    let parent: (CommonJsModule & { filename: string; paths: string[] }) | null = null;
    let required: NodeJS.Require | null = null;
    for (const { kind, specifier } of literalSpecifiers(code)) {
      let file: string | null;
      if (kind === "require") {
        required ??= nodeModule.createRequire(path);
        const from = required;
        file = resolvedBy(() => from.resolve(specifier));
      } else if (nodeModule.isBuiltin(specifier)) {
        file = null;
      } else if (/^(?:\.{1,2}(?:\/|$)|\/|file:)/.test(specifier)) {
        // A relative import names exactly its file, as Node resolves it: no extension is added.
        let named: string | null = null;
        try {
          named = filePath(new URL(specifier, pathToFileURL(path)).href);
        } catch {
          named = null;
        }
        file = named === null ? null : resolvedBy(() => (statSync(named).isFile() ? realpathSync(named) : ""));
      } else if (/^[A-Za-z][A-Za-z0-9+.-]*:/.test(specifier)) {
        file = null; // data:, http: and other schemes
      } else {
        // A `#` subpath import or a package name: Node's lookup from the file, as an import.
        if (parent === null) {
          parent = new LOADER(path);
          parent.filename = path;
          parent.paths = LOADER._nodeModulePaths(dirname(path));
        }
        const from = parent;
        const conditions = new Set(this.#importConditions ?? IMPORT_CONDITIONS);
        file = resolvedBy(() => LOADER._resolveFilename(specifier, from, false, { conditions }));
      }
      if (file === null) {
        continue;
      }
      const url = pathToFileURL(file).href;
      const entry = this.entry(url);
      if (entry !== null && this.#spelling(entry, listed) === null) {
        targets.add(url);
      }
    }
    return [...targets];
  }

  /** The closure entry of the file a module URL names, or null when it is not a source file. */
  entry(url: string): ClosureEntry | null {
    let entry = this.#entries.get(url);
    if (entry === undefined) {
      const path = filePath(url);
      const base = path === null ? null : this.#baseOf(path);
      if (path === null || base === null) {
        entry = null;
      } else {
        const inside = relative(base.folder, path).split(sep).join("/");
        entry = { label: base.prefix ? `${base.prefix}/${inside}` : inside, path };
      }
      this.#entries.set(url, entry);
    }
    return entry;
  }

  /** The first file of `entries` (in label order) not spelled as its directory entries spell it,
   * or null when every one is. */
  misspelling(entries: readonly ClosureEntry[]): Misspelling | null {
    const listed = new Map<string, readonly string[]>();
    for (const entry of entries) {
      const misspelled = this.#spelling(entry, listed);
      if (misspelled !== null) {
        return misspelled;
      }
    }
    return null;
  }

  /** How `entry` is misspelled, or null when every part of its path below its base is spelled as
   * its directory entry; `listed` keeps the folders read. */
  #spelling(entry: ClosureEntry, listed: Map<string, readonly string[]>): Misspelling | null {
    const list = (folder: string): readonly string[] => {
      let names = listed.get(folder);
      if (names === undefined) {
        try {
          names = readdirSync(folder);
        } catch {
          names = [];
        }
        listed.set(folder, names);
      }
      return names;
    };
    const base = this.#baseOf(entry.path);
    if (base === null) {
      return null;
    }
    const parts = relative(base.folder, entry.path).split(sep);
    const spelled: string[] = [];
    let folder = base.folder;
    let wrongCase = false;
    let wrongForm = false;
    for (const part of parts) {
      const names = list(folder);
      let name = part;
      if (!names.includes(part)) {
        const composed = part.normalize("NFC");
        const sameForm = names.find((candidate) => candidate.normalize("NFC") === composed);
        if (sameForm !== undefined) {
          name = sameForm;
          wrongForm = true;
        } else {
          const folded = composed.toLowerCase();
          const sameLetters = (candidate: string) => candidate.normalize("NFC").toLowerCase() === folded;
          name = names.find(sameLetters) ?? part;
          wrongCase = true;
        }
      }
      spelled.push(name);
      folder = join(folder, name);
    }
    if (!wrongCase && !wrongForm) {
      return null;
    }
    const prefix = base.prefix ? `${base.prefix}/` : "";
    return { imported: entry.label, onDisk: `${prefix}${spelled.join("/")}`, normalization: !wrongCase };
  }

  /** The base a file belongs to: the deepest of the root and the source packages' folders that
   * holds it with no `node_modules` folder below it. */
  #baseOf(path: string): Base | null {
    let found = this.#bases.get(path);
    if (found === undefined) {
      found = this.#findBase(path);
      this.#bases.set(path, found);
    }
    return found;
  }

  #findBase(path: string): Base | null {
    let found: Base | null = null;
    for (const base of [this.#root, ...this.#packages]) {
      if (base === null || !under(path, base.folder)) {
        continue;
      }
      const inside = relative(base.folder, path).split(sep);
      if (inside.slice(0, -1).includes("node_modules")) {
        continue;
      }
      if (found === null || base.folder.length > found.folder.length) {
        found = base;
      }
    }
    return found;
  }
}

function add(edges: Edges, from: string, to: string): void {
  let targets = edges.get(from);
  if (targets === undefined) {
    targets = new Set();
    edges.set(from, targets);
  }
  targets.add(to);
}

/** A module's source with the call that ends its top level appended on a new last line (module
 * comment): bytes stay bytes, so Node decodes them as it would (a byte order mark dropped). */
function marked(source: string | ArrayBuffer | ArrayBufferView, marker: string): string | Uint8Array {
  const call = `\n;${marker}(import.meta.url);`;
  if (typeof source === "string") {
    return `${source}${call}`;
  }
  const bytes =
    source instanceof ArrayBuffer
      ? new Uint8Array(source)
      : new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
  return Buffer.concat([bytes, Buffer.from(call, "utf8")]);
}

/** The files of the frames on the stack now, innermost first and at most `depth`, awaiting frames
 * included (URLs for ES modules, paths for CommonJS), or null when the stack cannot be read.
 * `Error.stackTraceLimit` and `Error.prepareStackTrace` are left as they were, each on its own. */
function stackFiles(depth: number): string[] | null {
  const limit = Error.stackTraceLimit;
  let sites: unknown;
  try {
    Error.stackTraceLimit = depth;
    const prepare = Error.prepareStackTrace;
    Error.prepareStackTrace = (_error, frames) => frames;
    try {
      const holder: { stack?: unknown } = {};
      Error.captureStackTrace(holder);
      sites = holder.stack;
    } finally {
      try {
        Error.prepareStackTrace = prepare;
      } catch {
        // as user code left it
      }
    }
  } catch {
    return null;
  } finally {
    try {
      Error.stackTraceLimit = limit;
    } catch {
      // as user code left it
    }
  }
  if (!Array.isArray(sites)) {
    return null;
  }
  const files: string[] = [];
  for (const site of sites as NodeJS.CallSite[]) {
    const file = site.getFileName();
    if (typeof file === "string") {
      files.push(file);
    }
  }
  return files;
}

/** The file of the innermost frame outside this module and Node's own (the code that called into
 * them), or null. */
function callerFile(): string | null {
  for (const file of stackFiles(CALLER_DEPTH) ?? []) {
    if (file !== OWN_URL && !file.startsWith("node:") && (file.startsWith("file:") || isAbsolute(file))) {
      return file;
    }
  }
  return null;
}

/** What `resolve` answers when it is an absolute path, else null (a builtin, a failure). */
function resolvedBy(resolve: () => string): string | null {
  try {
    const found = resolve();
    return isAbsolute(found) ? found : null;
  } catch {
    return null;
  }
}

/** `code` with its TypeScript types stripped as Node strips them when it loads the file, or null
 * when Node refuses it; Node's notice that the function is experimental is not printed. */
function stripTypes(code: string): string | null {
  const strip = (nodeModule as { stripTypeScriptTypes?: (code: string, options: { mode: "strip" }) => string })
    .stripTypeScriptTypes;
  if (typeof strip !== "function") {
    return null;
  }
  const emitWarning = process.emitWarning;
  let quieted = false;
  try {
    process.emitWarning = function (this: unknown, warning: unknown, ...rest: unknown[]): void {
      const kind = typeof rest[0] === "string" ? rest[0] : (rest[0] as { type?: unknown } | undefined)?.type;
      const text = warning instanceof Error ? warning.message : String(warning);
      if (kind === "ExperimentalWarning" && text.includes("stripTypeScriptTypes")) {
        return;
      }
      Reflect.apply(emitWarning, this, [warning, ...rest]);
    } as typeof process.emitWarning;
    quieted = true;
  } catch {
    // user code made it read-only
  }
  try {
    return strip(code, { mode: "strip" });
  } catch {
    return null;
  } finally {
    if (quieted) {
      try {
        process.emitWarning = emitWarning;
      } catch {
        // as it was
      }
    }
  }
}

function ownPackage(specifier: string): boolean {
  return specifier === OWN_PACKAGE || specifier.startsWith(`${OWN_PACKAGE}/`);
}

/** The file a module URL names (without its query or fragment), or null for one that names
 * none. */
function filePath(url: string): string | null {
  if (!url.startsWith("file:")) {
    return null;
  }
  try {
    const parsed = new URL(url);
    parsed.search = "";
    parsed.hash = "";
    return fileURLToPath(parsed);
  } catch {
    return null;
  }
}

/** Whether `path` lies under `folder` (not equal to it). */
function under(path: string, folder: string): boolean {
  return path.startsWith(folder.endsWith(sep) ? folder : folder + sep);
}

/** The real folder of the package `name`, as Node finds it from `root`: the first
 * `node_modules/<name>/package.json` from the root up. */
export function packageFolder(root: string, name: string): string | null {
  let folder = root;
  for (;;) {
    const candidate = join(folder, "node_modules", ...name.split("/"));
    try {
      if (statSync(join(candidate, "package.json")).isFile()) {
        return realpathSync(candidate);
      }
    } catch {
      // not here
    }
    const parent = dirname(folder);
    if (parent === folder) {
      return null;
    }
    folder = parent;
  }
}
