import { ViewerProtocol } from "../../../js/fx/src/viewer-protocol";
import { ViewerPresentationError } from "./presentation";
import { ExternalFileController, ExternalFileError } from "./external-file";

export interface ViewerPageSource {
  getContext(): ViewerProtocol.Context;
  present(action: ViewerProtocol.Action): ViewerProtocol.Context;
  subscribe(listener: () => void): () => void;
}
interface FileOwner {
  peer: Window;
  origin: string;
  handlerId: string;
  sequence: number;
  release(): void;
  pending: Map<number, { finish(ok: boolean): void }>;
}

/** The page-level postMessage endpoint. Window references are the browser's identities. */
export class ViewerPage {
  private readonly grants = new Map<Window, Set<string>>();
  private readonly peers = new Map<Window, Set<string>>();
  private readonly listeners = new Set<(context: ViewerProtocol.Context) => void>();
  private readonly detach: () => void;
  private queued = false;
  private disposed = false;
  private previous = "";
  private fileOwner: FileOwner | null = null;

  constructor(private readonly window: Window, private readonly source: ViewerPageSource, private readonly files?: ExternalFileController) {
    window.addEventListener("message", this.onMessage);
    this.detach = source.subscribe(this.changed);
  }

  /** Called by trusted code in this realm, never by a window message. */
  allowHost = (host: Window, origin: string): (() => void) => {
    const url = new URL(origin);
    if (!["http:", "https:"].includes(url.protocol) || url.origin !== origin || !this.isWindow(host)) throw new Error("Supply an exact host window and HTTP(S) origin.");
    const origins = this.grants.get(host) ?? new Set<string>();
    origins.add(origin);
    this.grants.set(host, origins);
    return () => {
      origins.delete(origin);
      if (origins.size === 0) this.grants.delete(host);
      this.peers.get(host)?.delete(origin);
      if (this.fileOwner?.peer === host && this.fileOwner.origin === origin) this.releaseFileOwner(this.fileOwner);
    };
  };

  getContext = (): ViewerProtocol.Context => this.source.getContext();
  present = (action: ViewerProtocol.Action): ViewerProtocol.Context => this.source.present(action);
  subscribe = (listener: (context: ViewerProtocol.Context) => void): (() => void) => {
    const context = this.getContext();
    this.listeners.add(listener);
    try { listener(context); } catch { /* A consumer cannot interrupt the page. */ }
    return () => { this.listeners.delete(listener); };
  };

  private isWindow(source: unknown): source is Window {
    try { return !!source && (source as Window).window === source; } catch { return false; }
  }
  private allowed(source: Window, origin: string): boolean {
    return /^https?:\/\//.test(origin) && (origin === this.window.location.origin || this.grants.get(source)?.has(origin) === true);
  }
  private send(target: Window, origin: string, message: ViewerProtocol.Response | ViewerProtocol.ContextEvent | ViewerProtocol.ExternalFileEvent): boolean {
    try {
      if (target.closed || !this.allowed(target, origin)) return false;
      target.postMessage(message, origin);
      return true;
    } catch { return false; }
  }
  private registerFileOwner(peer: Window, origin: string, handlerId: string): void {
    if (!this.files) throw new ViewerPresentationError("unavailable", "This viewer cannot delegate external files.");
    if (this.fileOwner) {
      if (this.fileOwner.peer === peer && this.fileOwner.origin === origin && this.fileOwner.handlerId === handlerId) return;
      if (this.fileOwner.peer !== peer || this.fileOwner.origin !== origin) {
        throw new ViewerPresentationError("unavailable", "An external-file handler is already registered.");
      }
      // The same authorized peer can reconnect; stale cleanup retains its old token.
      this.releaseFileOwner(this.fileOwner);
    }
    const owner: FileOwner = { peer, origin, handlerId, sequence: 0, release: () => {}, pending: new Map() };
    owner.release = this.files.handleExternalFile((reference) => this.forwardFile(owner, reference));
    this.fileOwner = owner;
  }
  private forwardFile(owner: FileOwner, reference: ViewerProtocol.FileReference): Promise<void> {
    if (this.fileOwner !== owner || !this.allowed(owner.peer, owner.origin)) return Promise.reject(new Error("File handler unavailable."));
    const activation_id = ++owner.sequence;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => pending.finish(false), 10_000);
      const pending = { finish: (ok: boolean) => {
        if (!owner.pending.delete(activation_id)) return;
        clearTimeout(timer);
        if (ok) resolve(); else reject(new Error("File handling was not confirmed."));
      } };
      owner.pending.set(activation_id, pending);
      if (!this.send(owner.peer, owner.origin, {
        channel: ViewerProtocol.CHANNEL, version: ViewerProtocol.VERSION, type: "external_file",
        handler_id: owner.handlerId, activation_id, reference,
      })) this.releaseFileOwner(owner);
    });
  }
  private releaseFileOwner(owner: FileOwner): void {
    if (this.fileOwner !== owner) return;
    this.fileOwner = null;
    for (const pending of [...owner.pending.values()]) pending.finish(false);
    owner.release();
  }
  private onMessage = (event: MessageEvent): void => {
    if (this.disposed || !this.isWindow(event.source) || !this.allowed(event.source, event.origin)) return;
    const request = event.data;
    if (!ViewerProtocol.isRequest(request)) return;
    const base = { channel: ViewerProtocol.CHANNEL, version: ViewerProtocol.VERSION, type: "response", request_id: request.request_id } as const;
    try {
      if (request.version !== ViewerProtocol.VERSION) throw new ViewerPresentationError("unsupported_version", "This viewer supports page protocol version 1.");
      if (!ViewerProtocol.isMethod(request.method)) throw new ViewerPresentationError("unsupported_method", "This viewer does not support that page method.");
      if (!ViewerProtocol.isParams(request.method, request.params)) throw new ViewerPresentationError("invalid_request", "The page method parameters are malformed.");
      let result: ViewerProtocol.Context | null;
      switch (request.method) {
        case "unsubscribe": {
          const origins = this.peers.get(event.source);
          origins?.delete(event.origin);
          if (origins?.size === 0) this.peers.delete(event.source);
          result = null;
          break;
        }
        case "subscribe": {
          result = this.getContext();
          const origins = this.peers.get(event.source) ?? new Set<string>();
          origins.add(event.origin);
          this.peers.set(event.source, origins);
          break;
        }
        case "get_context": result = this.getContext(); break;
        case "present": result = this.present(request.params as ViewerProtocol.Action); break;
        case "register_external_file_handler":
          this.registerFileOwner(event.source, event.origin, request.params.handler_id as string);
          result = null;
          break;
        case "unregister_external_file_handler": {
          const owner = this.fileOwner;
          if (owner?.peer === event.source && owner.origin === event.origin && owner.handlerId === request.params.handler_id) this.releaseFileOwner(owner);
          result = null;
          break;
        }
        case "external_file_result": {
          const owner = this.fileOwner;
          if (owner?.peer === event.source && owner.origin === event.origin && owner.handlerId === request.params.handler_id) {
            owner.pending.get(request.params.activation_id as number)?.finish(request.params.ok as boolean);
          }
          result = null;
          break;
        }
      }
      this.send(event.source, event.origin, { ...base, ok: true, result });
    } catch (error) {
      const expected = error instanceof ViewerPresentationError || error instanceof ExternalFileError;
      this.send(event.source, event.origin, { ...base, ok: false, error: {
        code: expected ? error.code : "unavailable",
        message: expected ? error.message : "The viewer could not apply the page request.",
      } });
    }
  };
  private changed = (): void => {
    if (this.queued || this.disposed) return;
    this.queued = true;
    queueMicrotask(() => {
      this.queued = false;
      if (this.disposed) return;
      let context: ViewerProtocol.Context;
      try { context = this.getContext(); } catch { return; }
      const signature = JSON.stringify({ ...context, sampled_at: 0 });
      if (signature === this.previous) return;
      this.previous = signature;
      for (const listener of [...this.listeners]) {
        try { listener(structuredClone(context)); } catch { /* Isolate local consumers. */ }
      }
      for (const [peer, origins] of this.peers) {
        for (const origin of origins) if (!this.send(peer, origin, {
          channel: ViewerProtocol.CHANNEL, version: ViewerProtocol.VERSION, type: "context_changed", context,
        })) origins.delete(origin);
        if (origins.size === 0) this.peers.delete(peer);
      }
    });
  };

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    if (this.fileOwner) this.releaseFileOwner(this.fileOwner);
    this.window.removeEventListener("message", this.onMessage);
    this.detach();
    this.peers.clear();
    this.grants.clear();
    this.listeners.clear();
  }
}
