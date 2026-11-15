/** Read-only discovery and native resolution of recorded files. Paths returned here belong to
 * the trusted native caller; they must not be forwarded into a browser document or message. */

import { isAbsolute, resolve } from "node:path";
import { call, type CallOptions, type Exit, failure } from "./engine.js";
import { FxError } from "./errors.js";
import { ViewerProtocol } from "./viewer-protocol.js";

export type FileTarget = ViewerProtocol.FileTarget;
export type FileReference = ViewerProtocol.FileReference;

export interface FileAccessOptions extends CallOptions {
  /** Explicit source/project binding. Source resolution never infers this from cwd. */
  readonly project?: string;
}

export interface ListFilesOptions extends FileAccessOptions {
  /** Only recorded files associated with this exact instance. */
  readonly instanceId?: string;
}

export interface ResolveFileOptions extends FileAccessOptions {
  /** Explicit base for the complete source label. Required for closure-only helpers,
   * and when the owning source root differs from the planning project's binding. */
  readonly sourceRoot?: string;
}

export interface FileDescriptor {
  readonly target: FileTarget;
  readonly role: "artifact" | "workflow" | "node" | "source" | "resource";
  readonly instanceId?: string;
  readonly scopeId?: string;
}

export interface FileResolution {
  readonly target: FileTarget;
  /** An available local placement. This is private native data, not a browser URL. */
  readonly localPath: string | null;
  /** Source is the current editable file; changed source remains openable. */
  readonly sourceMatch: "match" | "changed" | "unknown" | null;
  readonly reason: string | null;
}

export interface FileAccessFailure {
  readonly kind: "fx-file-error-v1";
  readonly code: string;
  readonly message: string;
  readonly [field: string]: unknown;
}

/** A structured file-reader refusal; unknown future codes remain available to callers. */
export class FileAccessError extends FxError {
  readonly code: string;
  readonly document: FileAccessFailure;

  constructor(document: FileAccessFailure, done: Exit) {
    super(document.message, {
      exitCode: done.status, signal: done.signal, stdout: done.stdout,
      stderr: done.stderr, args: done.args,
    });
    this.name = "FileAccessError";
    this.code = document.code;
    this.document = document;
  }
}

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function text(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && !value.includes("\0");
}

function targetArgument(target: string): string {
  if (!text(target) || target.startsWith("-")) {
    throw new TypeError("target is a run folder or saved plan file without a leading '-' or NUL");
  }
  return target;
}

function projectArguments(options: FileAccessOptions): string[] {
  if (options.project === undefined) return [];
  if (!text(options.project)) throw new TypeError("project is a nonempty filesystem path without NUL");
  return [`--project=${resolve(options.cwd ?? process.cwd(), options.project)}`];
}

function document(done: Exit, kind: string, verb: string): Record<string, unknown> {
  let value: unknown;
  try { value = JSON.parse(done.stdout); } catch { throw failure(verb, done); }
  if (done.status === 2 && object(value) && value.kind === "fx-file-error-v1"
    && text(value.code) && typeof value.message === "string") {
    throw new FileAccessError(value as unknown as FileAccessFailure, done);
  }
  if (done.status !== 0 || !object(value) || value.kind !== kind) throw failure(verb, done);
  return value;
}

/** Enumerate recorded artifacts and safe source references without executing author code.
 * `target` is an explicit run folder or saved plan JSON file, never a latest-run selector. */
export async function listFiles(target: string, options: ListFilesOptions = {}): Promise<FileDescriptor[]> {
  const args = ["files", "list", targetArgument(target), "--json", ...projectArguments(options)];
  if (options.instanceId !== undefined) {
    if (!ViewerProtocol.isIdentifier(options.instanceId)) throw new TypeError("instanceId is a recorded instance identifier");
    args.push(`--instance=${options.instanceId}`);
  }
  const done = await call(args, options);
  const value = document(done, "fx-files-v1", "files list");
  if (!Array.isArray(value.files)) throw failure("files list", done);
  return value.files.map((entry: unknown): FileDescriptor => {
    if (!object(entry) || !ViewerProtocol.isFileTarget(entry.target)
      || !["artifact", "workflow", "node", "source", "resource"].includes(entry.role as string)
      || (entry.instance_id !== undefined && !ViewerProtocol.isIdentifier(entry.instance_id))
      || (entry.scope_id !== undefined && !ViewerProtocol.isIdentifier(entry.scope_id))) {
      throw failure("files list", done);
    }
    return {
      target: entry.target, role: entry.role as FileDescriptor["role"],
      ...(entry.instance_id !== undefined ? { instanceId: entry.instance_id as string } : {}),
      ...(entry.scope_id !== undefined ? { scopeId: entry.scope_id as string } : {}),
    };
  });
}

/** Resolve the exact captured file reference through the engine. Artifacts are verified;
 * sources are current editable files and require an explicit local source binding. */
export async function resolveFile(target: string, reference: FileReference, options: ResolveFileOptions = {}): Promise<FileResolution> {
  const selected = targetArgument(target);
  if (!ViewerProtocol.isFileReference(reference)) throw new TypeError("reference is a portable viewer file reference");
  const args = ["files", "resolve", selected, "--reference", JSON.stringify(reference), "--json", ...projectArguments(options)];
  if (options.sourceRoot !== undefined) {
    if (!text(options.sourceRoot)) throw new TypeError("sourceRoot is a nonempty filesystem path without NUL");
    args.push(`--source-root=${resolve(options.cwd ?? process.cwd(), options.sourceRoot)}`);
  }
  const done = await call(args, options);
  const value = document(done, "fx-file-resolution-v1", "files resolve");
  if (!ViewerProtocol.isFileTarget(value.target)
    || !(value.local_path === null || (text(value.local_path) && isAbsolute(value.local_path)))
    || ![null, "match", "changed", "unknown"].includes(value.source_match as null | string)
    || !(value.reason === null || typeof value.reason === "string")) {
    throw failure("files resolve", done);
  }
  return {
    target: value.target, localPath: value.local_path,
    sourceMatch: value.source_match as FileResolution["sourceMatch"], reason: value.reason,
  };
}
