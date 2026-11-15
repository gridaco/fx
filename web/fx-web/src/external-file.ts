import { ViewerProtocol } from "../../../js/fx/src/viewer-protocol";

export interface ExternalFileState {
  available: boolean;
  pending: number;
  error: string | null;
  failedReference: ViewerProtocol.FileReference | null;
}
export interface FileActivationEvent {
  readonly button?: number;
  readonly ctrlKey?: boolean;
  readonly metaKey?: boolean;
  readonly shiftKey?: boolean;
  readonly altKey?: boolean;
  readonly defaultPrevented?: boolean;
  preventDefault(): void;
}
export class ExternalFileError extends Error {
  constructor(readonly code: "unavailable" | "invalid_request", message: string) {
    super(message);
    this.name = "ExternalFileError";
  }
}
interface PendingActivation { owner: object; finish(error: string | null): void }

/** File behavior injection shared by the supplied viewer and independently composed UIs. */
export class ExternalFileController {
  private state: ExternalFileState = { available: false, pending: 0, error: null, failedReference: null };
  private readonly listeners = new Set<() => void>();
  private readonly pending = new Set<PendingActivation>();
  private owner: { handler: ViewerProtocol.ExternalFileHandler } | null = null;
  private readonly timeoutMs: number;
  private disposed = false;

  constructor(private readonly getContext: () => ViewerProtocol.Context, options: { timeoutMs?: number } = {}) {
    this.timeoutMs = options.timeoutMs ?? 10_000;
    if (!Number.isFinite(this.timeoutMs) || this.timeoutMs <= 0 || this.timeoutMs > 2_147_483_647) {
      throw new ExternalFileError("invalid_request", "Supply a positive finite file-handler timeout.");
    }
  }
  getSnapshot = (): ExternalFileState => this.state;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private update(next: Partial<ExternalFileState>): void {
    this.state = { ...this.state, ...next };
    for (const listener of [...this.listeners]) {
      try { listener(); } catch { /* UI listeners cannot interrupt the file owner. */ }
    }
  }
  handleExternalFile = (handler: ViewerProtocol.ExternalFileHandler): (() => void) => {
    if (this.disposed || this.owner) throw new ExternalFileError("unavailable", "An external-file handler is unavailable or already registered.");
    if (typeof handler !== "function") throw new ExternalFileError("invalid_request", "Supply an external-file handler.");
    const owner = { handler };
    this.owner = owner;
    this.update({ available: true, error: null, failedReference: null });
    return () => {
      if (this.owner !== owner) return;
      this.owner = null;
      for (const activation of [...this.pending]) if (activation.owner === owner) activation.finish("File handling was not confirmed.");
      this.update({ available: false });
    };
  };
  /** Returns true only when the registered owner takes the ordinary link activation. */
  activate = (target: ViewerProtocol.FileTarget, event?: FileActivationEvent): boolean => {
    const owner = this.owner;
    if (this.disposed || !owner || event?.defaultPrevented || event?.button !== undefined && event.button !== 0
      || event?.ctrlKey || event?.metaKey || event?.shiftKey || event?.altKey) return false;
    event?.preventDefault();
    let reference: ViewerProtocol.FileReference;
    try {
      reference = structuredClone({ subject: this.getContext().subject, target });
      if (!ViewerProtocol.isFileReference(reference)) throw new Error();
    } catch {
      this.update({ error: "This file is unavailable for external handling.", failedReference: null });
      return true;
    }
    if (this.pending.size >= 64) {
      this.update({ error: "Too many file activations are awaiting confirmation.", failedReference: reference });
      return true;
    }
    let timer: ReturnType<typeof setTimeout>;
    const activation: PendingActivation = {
      owner,
      finish: (error) => {
        if (!this.pending.delete(activation)) return;
        clearTimeout(timer);
        if (!this.disposed) this.update({ pending: this.pending.size, error, failedReference: error ? reference : null });
      },
    };
    timer = setTimeout(() => activation.finish("File handling was not confirmed before the timeout."), this.timeoutMs);
    this.pending.add(activation);
    this.update({ pending: this.pending.size, error: null, failedReference: null });
    // Promise callbacks isolate synchronous exceptions and never replay browser navigation.
    void Promise.resolve().then(() => {
      if (!this.pending.has(activation) || this.owner !== owner || this.disposed) return;
      return owner.handler(structuredClone(reference));
    }).then(() => activation.finish(null), () => activation.finish("The host could not handle this file."));
    return true;
  };
  clearError = (): void => {
    if (!this.disposed && this.state.error !== null) this.update({ error: null, failedReference: null });
  };
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.owner = null;
    for (const activation of [...this.pending]) activation.finish(null);
    this.state = { available: false, pending: 0, error: null, failedReference: null };
    this.listeners.clear();
  }
}
