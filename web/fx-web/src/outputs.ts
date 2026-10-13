import { Zip, ZipPassThrough } from "fflate";

/** One file of a run's declared outputs. */
export interface OutputFile {
  /** Its name in the run folder's `outputs/` (spec/store.md §8 "Names"), before [`placedName`] cuts long segments. */
  name: string;
  digest: string;
  kind: string;
  size: number;
  /** Where the viewer serves its bytes; null when it cannot. */
  url: string | null;
}

/** The first suffix identity.md §4 lists for each kind, the one a run folder writes; `annotations` takes `.json`. */
const SUFFIXES = new Map(Object.entries({
  "image/png": ".png", "image/jpeg": ".jpg", "image/webp": ".webp", "image/gif": ".gif",
  "text/markdown": ".md", "text/plain": ".txt", json: ".json", annotations: ".json", "text/yaml": ".yaml", "text/toml": ".toml", "text/html": ".html",
  "audio/wav": ".wav", "audio/mpeg": ".mp3", "audio/ogg": ".ogg", "video/mp4": ".mp4", "video/webm": ".webm", "video/x-matroska": ".mkv",
  "model/gltf-binary": ".glb", "model/gltf+json": ".gltf", "model/fbx": ".fbx", "file/zip": ".zip",
}));
/** What one file-system name holds, in UTF-8 bytes (store.md §8 "Length"). */
const NAME_BYTES = 255;
/** A zip without ZIP64 holds at most 4 GiB, and a tab often cannot hold half that. */
const ARCHIVE_LIMIT = 2 * 1024 ** 3;
const TOO_LARGE = "These outputs are too large for this tab to put in one zip. Save each output from its own link.";

const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const utf8 = new TextEncoder();

/** A step path as one name: anything but a letter, a digit, `.`, `_` or `-` becomes `_`, outer `_` go, empty is `step`. */
function safeName(text: string) {
  return [...text].map((char) => /[\p{L}\p{N}._-]/u.test(char) ? char : "_").join("").replace(/^_+|_+$/g, "") || "step";
}

/** A key as a path: split at `/`, an empty, `.` or `..` segment becomes `_`, every other one a [`safeName`]. */
function keyedPath(key: string) {
  return key.split("/").map((segment) => ["", ".", ".."].includes(segment) ? "_" : safeName(segment)).join("/");
}

function named(label: string, kind: string) {
  const suffix = SUFFIXES.get(kind) ?? "";
  return !suffix || label.endsWith(suffix) ? label : label + suffix;
}

async function sha256(bytes: Uint8Array<ArrayBuffer>) {
  return Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), (part) => part.toString(16).padStart(2, "0")).join("");
}

/**
 * Each segment of `name` cut to one file-system name, as the run folder cuts it: a longer one keeps
 * its first whole characters, then `~`, 16 hex characters of its SHA-256 and, last, its suffix.
 */
export async function placedName(file: Pick<OutputFile, "name" | "kind">) {
  const suffix = SUFFIXES.get(file.kind) ?? "";
  const segments = file.name.split("/");
  return (await Promise.all(segments.map(async (segment, index) => {
    const bytes = utf8.encode(segment);
    if (bytes.length <= NAME_BYTES) return segment;
    const kept = index === segments.length - 1 && segment.endsWith(suffix) ? suffix : "";
    let stem = "", room = NAME_BYTES - 1 - 16 - kept.length;
    for (const char of segment.slice(0, segment.length - kept.length)) {
      room -= utf8.encode(char).length;
      if (room < 0) break;
      stem += char;
    }
    return `${stem}~${(await sha256(bytes)).slice(0, 16)}${kept}`;
  }))).join("/");
}

/** A value's files in order (a list's items, a collection's in collection order), each with its key or its item's. */
function filesOf(value: unknown, itemKey: string | null, found: { file: Record<string, unknown>; key: string | null }[]) {
  if (!record(value)) return found;
  if (record(value.file)) {
    const { file } = value;
    if (typeof file.digest === "string" && /^[a-f0-9]{64}$/.test(file.digest) && typeof file.kind === "string" && Number.isSafeInteger(file.size)) {
      found.push({ file, key: (typeof file.key === "string" && file.key) || itemKey || null });
    }
  } else if (Array.isArray(value.list)) {
    for (const item of value.list) filesOf(item, null, found);
  } else if (Array.isArray(value.collection)) {
    for (const entry of value.collection) if (Array.isArray(entry) && entry.length === 2 && typeof entry[0] === "string") filesOf(entry[1], entry[0], found);
  }
  return found;
}

/**
 * The files of a run's declared outputs, named as its run folder's `outputs/` names them: an output
 * that is one file is `<name><suffix>`; each file of any other is `<name>/<label><suffix>`, its label
 * its key as a path, else its position among the output's files. `locate` gives a file's URL.
 */
export function outputFiles(outputs: Record<string, unknown>, locate: (digest: string) => string | null): OutputFile[] {
  return Object.entries(outputs).flatMap(([output, value]) => filesOf(value, null, []).map(({ file, key }, index) => ({
    name: named(record(value) && record(value.file) ? output : `${output}/${key === null ? index : keyedPath(key)}`, file.kind as string),
    digest: file.digest as string,
    kind: file.kind as string,
    size: file.size as number,
    url: locate(file.digest as string),
  })));
}

export interface ArchiveOptions { fetcher?: typeof fetch; signal?: AbortSignal; limit?: number }

/**
 * The files that have a URL in one uncompressed zip (media is compressed already), each checked
 * against its recorded size and digest. A later file at a name replaces an earlier one, as in the
 * run folder. Refuses, before reading anything, files that come to more than `limit` bytes.
 */
export async function outputArchive(files: OutputFile[], { fetcher = fetch, signal, limit = ARCHIVE_LIMIT }: ArchiveOptions = {}): Promise<Blob> {
  const entries = new Map<string, OutputFile>();
  for (const file of files) if (file.url !== null) { const name = await placedName(file); entries.delete(name); entries.set(name, file); }
  const total = [...entries.values()].reduce((sum, file) => sum + file.size, 0);
  if (total > limit) throw new Error(`These outputs come to ${(total / 1024 ** 3).toFixed(1)} GB, more than this tab can put in one zip. Save each output from its own link.`);
  const chunks: Uint8Array<ArrayBuffer>[] = [];
  const failures: Error[] = [];
  const zip = new Zip((error, chunk) => { if (error) failures.push(error); else chunks.push(chunk); });
  for (const [name, file] of entries) {
    const response = await fetcher(file.url!, { signal });
    if (!response.ok) throw new Error(`${file.name} could not be read (HTTP ${response.status}).`);
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.length !== file.size || await sha256(bytes) !== file.digest) throw new Error(`${file.name} is not the file the run recorded.`);
    const entry = new ZipPassThrough(name);
    zip.add(entry);
    entry.push(bytes, true);
  }
  zip.end();
  if (failures.length) throw failures[0];
  return new Blob(chunks, { type: "application/zip" });
}

export interface OutputArchiveState { building: boolean; error: string | null }

/** Puts a run's output files in one zip and saves it; the viewer only mounts it. */
export class OutputArchiveController {
  private state: OutputArchiveState = { building: false, error: null };
  private listeners = new Set<() => void>();
  private request: AbortController | null = null;

  constructor(private readonly options: Omit<ArchiveOptions, "signal"> & { save?: (archive: Blob, name: string) => void } = {}) {}

  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(next: OutputArchiveState) { this.state = next; this.listeners.forEach((listener) => listener()); }

  /** Saves `files` as `<name>.zip`. */
  download = async (files: OutputFile[], name: string) => {
    this.request?.abort();
    const request = new AbortController();
    this.request = request;
    this.update({ building: true, error: null });
    try {
      const archive = await outputArchive(files, { ...this.options, signal: request.signal });
      if (request.signal.aborted) return;
      (this.options.save ?? save)(archive, `${name}.zip`);
      this.update({ building: false, error: null });
    } catch (error) {
      if (request.signal.aborted) return;
      this.update({ building: false, error: error instanceof RangeError ? TOO_LARGE : error instanceof Error ? error.message : "The outputs could not be zipped." });
    }
  };

  dispose() { this.request?.abort(); }
}

function save(archive: Blob, name: string) {
  const link = Object.assign(document.createElement("a"), { href: URL.createObjectURL(archive), download: name });
  document.body.append(link);
  link.click();
  link.remove();
  // Revoked later: some browsers still read the URL after the click returns.
  setTimeout(() => URL.revokeObjectURL(link.href), 60_000);
}
