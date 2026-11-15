import { ViewerProtocol } from "../../../js/fx/src/viewer-protocol";
import type { Artifact } from "./index";
import { validateViewerApiBase } from "./route";

export type FileTarget = ViewerProtocol.FileTarget;
export type FileReference = ViewerProtocol.FileReference;

export interface FileDescriptor {
  target: FileTarget;
  role: "artifact" | "workflow" | "node" | "source" | "resource";
  instance_id?: string;
  scope_id?: string;
}

/** The recorded artifact reference is independent of its optional byte-serving URL. */
export function artifactFileTarget(artifact: Artifact): FileTarget {
  const { digest, url, name, kind, size } = artifact;
  // Match native file discovery: unsafe recorded labels use the digest in portable references.
  const portableName = /^[\\/]|^[A-Za-z]:|^file:|[\u0000-\u001f\u007f-\u009f]/i.test(name) ? digest : name;
  return { kind: "artifact", artifact: { digest, url, name: portableName, kind, size } };
}

const roles = new Set(["artifact", "workflow", "node", "source", "resource"]);

/** Local viewer metadata contains portable references only, never native resolution results. */
export function parseFileInventory(value: unknown, apiBase = "/api"): FileDescriptor[] {
  validateViewerApiBase(apiBase);
  if (!ViewerProtocol.record(value) || value.kind !== "fx-files-v1" || !Array.isArray(value.files)) throw new Error("This viewer does not support the file inventory.");
  const files = value.files;
  for (const item of files) {
    if (!ViewerProtocol.record(item) || !ViewerProtocol.isFileTarget(item.target)
      || Object.keys(item).some((key) => !["target", "role", "instance_id", "scope_id"].includes(key))
      || typeof item.role !== "string" || !roles.has(item.role)
      || ["instance_id", "scope_id"].some((key) => key in item && !ViewerProtocol.isIdentifier(item[key]))) throw new Error("The viewer file inventory is malformed.");
    if (item.target.kind === "artifact") {
      const { digest, url } = item.target.artifact;
      if (url !== null && url !== `${apiBase}/artifacts/${digest}`) throw new Error("The file inventory contains an unscoped artifact URL.");
    }
  }
  return structuredClone(files) as FileDescriptor[];
}

export async function readFileInventory(signal?: AbortSignal, apiBase = "/api"): Promise<FileDescriptor[]> {
  const response = await fetch(`${validateViewerApiBase(apiBase)}/files`, { signal, cache: "no-store" });
  if (!response.ok) throw new Error("The recorded file inventory is unavailable.");
  return parseFileInventory(await response.json(), apiBase);
}

export interface FileInventoryState { files: FileDescriptor[]; loading: boolean; error: string | null }

/** On-demand source discovery. Reads are cancellable and a stale response never replaces a newer one. */
export class FileInventoryController {
  private state: FileInventoryState = { files: [], loading: false, error: null };
  private readonly listeners = new Set<() => void>();
  private request: AbortController | null = null;
  private disposed = false;

  constructor(private readonly read: (signal: AbortSignal) => Promise<FileDescriptor[]> = readFileInventory) {}

  getSnapshot = (): FileInventoryState => this.state;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private update(state: FileInventoryState): void {
    this.state = state;
    for (const listener of this.listeners) listener();
  }
  refresh = async (): Promise<void> => {
    if (this.disposed) return;
    this.request?.abort();
    const request = new AbortController();
    this.request = request;
    this.update({ ...this.state, loading: true, error: null });
    try {
      const files = await this.read(request.signal);
      if (request.signal.aborted || this.disposed) return;
      this.update({ files, loading: false, error: null });
    } catch {
      if (request.signal.aborted || this.disposed) return;
      this.update({ files: [], loading: false, error: "The recorded source files are unavailable." });
    } finally {
      if (this.request === request) this.request = null;
    }
  };
  clear = (): void => {
    this.request?.abort(); this.request = null;
    this.update({ files: [], loading: false, error: null });
  };
  dispose(): void {
    this.disposed = true; this.request?.abort(); this.listeners.clear();
  }
}
