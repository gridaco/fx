import { ViewerProtocol } from "../../../js/fx/src/viewer-protocol.js";
import { ViewerPresentationError } from "./presentation";

export interface ViewerPageSource {
  getContext(): ViewerProtocol.Context;
  present(action: ViewerProtocol.Action): ViewerProtocol.Context;
  subscribe(listener: () => void): () => void;
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

  constructor(private readonly window: Window, private readonly source: ViewerPageSource) {
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
  private send(target: Window, origin: string, message: ViewerProtocol.Response | ViewerProtocol.ContextEvent): boolean {
    try {
      if (target.closed || !this.allowed(target, origin)) return false;
      target.postMessage(message, origin);
      return true;
    } catch { return false; }
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
      }
      this.send(event.source, event.origin, { ...base, ok: true, result });
    } catch (error) {
      const expected = error instanceof ViewerPresentationError;
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
    this.window.removeEventListener("message", this.onMessage);
    this.detach();
    this.peers.clear();
    this.grants.clear();
    this.listeners.clear();
  }
}
