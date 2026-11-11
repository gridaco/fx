import { ViewerProtocol } from "../viewer-protocol.js";

export type ViewerContext = ViewerProtocol.Context;
export type ViewerAction = ViewerProtocol.Action;
export type ViewerSelection = ViewerProtocol.Selection;
export type ViewerSubject = ViewerProtocol.Subject;
export type ViewerReference = ViewerProtocol.Reference;
export type ViewerProtocolError = ViewerProtocol.Error;

/** Structural browser interfaces keep this subpath's declarations free of ambient DOM/Node types. */
export interface ViewerMessageEvent { readonly data: unknown; readonly origin: string; readonly source: unknown }
export interface ViewerTargetWindow { postMessage(message: unknown, targetOrigin: string): void }
export interface ViewerSourceWindow {
  addEventListener(type: "message", listener: (event: ViewerMessageEvent) => void): void;
  removeEventListener(type: "message", listener: (event: ViewerMessageEvent) => void): void;
}
export interface ViewerClientOptions {
  targetWindow: ViewerTargetWindow;
  origin: string;
  sourceWindow?: ViewerSourceWindow;
  timeoutMs?: number;
}

export class ViewerClientError extends Error {
  constructor(readonly code: string, message: string) {
    super(message);
    this.name = "ViewerClientError";
  }
}

interface TimerHost {
  setTimeout(handler: () => void, milliseconds: number): unknown;
  clearTimeout(handle: unknown): void;
}
interface Pending {
  method: ViewerProtocol.Method;
  resolve: (value: ViewerContext | null) => void;
  reject: (error: ViewerClientError) => void;
  timer: unknown;
}
interface Listener { callback: (context: ViewerContext) => void; initialized: boolean }
let nextClient = 0;

/**
 * A client for one existing viewer window; it never loads or starts FX.
 * Share one client per source/target window pair: local listeners multiplex the
 * protocol's single subscription for that peer rather than competing clients.
 */
export class ViewerClient {
  private readonly target: ViewerTargetWindow;
  private readonly source: ViewerSourceWindow;
  private readonly origin: string;
  private readonly timeoutMs: number;
  private readonly prefix = `fx-viewer-client-${++nextClient}-`;
  private nextRequest = 0;
  private readonly pending = new Map<string, Pending>();
  private readonly listeners = new Set<Listener>();
  private disposed = false;
  private subscribed = false;
  private subscriptionPending: Promise<ViewerContext> | null = null;
  private unsubscriptionPending: Promise<void> | null = null;
  private remoteSubscriptionPossible = false;
  private context: ViewerContext | null = null;
  private contextEvents = 0;

  constructor(options: ViewerClientOptions) {
    try {
      const parsed = new URL(options.origin);
      if (!["http:", "https:"].includes(parsed.protocol) || parsed.origin !== options.origin) throw new Error();
    } catch {
      throw new ViewerClientError("invalid_origin", "Supply an exact HTTP(S) viewer origin.");
    }
    const timeoutMs = options.timeoutMs ?? 10_000;
    if (!Number.isFinite(timeoutMs) || timeoutMs <= 0 || timeoutMs > 2_147_483_647) {
      throw new ViewerClientError("invalid_options", "timeoutMs must be a positive finite timer duration.");
    }
    const source = options.sourceWindow ?? (globalThis as { window?: ViewerSourceWindow }).window;
    if (!source || typeof source.addEventListener !== "function" || typeof source.removeEventListener !== "function"
      || !options.targetWindow || typeof options.targetWindow.postMessage !== "function") {
      throw new ViewerClientError("invalid_options", "Supply a viewer window and an available browser message source.");
    }
    this.target = options.targetWindow;
    this.source = source;
    this.origin = options.origin;
    this.timeoutMs = timeoutMs;
    this.source.addEventListener("message", this.onMessage);
  }

  async getContext(): Promise<ViewerContext> {
    return (await this.request("get_context", {}))!;
  }

  async present(action: ViewerAction): Promise<ViewerContext> {
    if (!ViewerProtocol.isAction(action)) throw new ViewerClientError("invalid_request", "Unknown or malformed presentation action.");
    return (await this.request("present", action))!;
  }

  async subscribe(callback: (context: ViewerContext) => void): Promise<() => void> {
    this.assertOpen();
    if (typeof callback !== "function") throw new ViewerClientError("invalid_request", "A context listener is required.");
    const listener: Listener = { callback, initialized: false };
    this.listeners.add(listener);
    try {
      const context = await this.ensureSubscribed();
      this.assertOpen();
      listener.initialized = true;
      this.notify(listener, context);
    } catch (error) {
      this.listeners.delete(listener);
      throw error;
    }
    let active = true;
    return () => {
      if (!active) return;
      active = false;
      this.listeners.delete(listener);
      if (this.listeners.size === 0) this.unsubscribeRemote();
    };
  }

  /** Release only this client's messaging resources. Viewer and workflow lifetimes are unchanged. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.source.removeEventListener("message", this.onMessage);
    this.listeners.clear();
    for (const [id, pending] of this.pending) {
      this.timers.clearTimeout(pending.timer);
      this.pending.delete(id);
      pending.reject(new ViewerClientError("disposed", "The viewer client has been disposed."));
    }
    if (this.remoteSubscriptionPossible) this.unsubscribeBestEffort();
    this.subscribed = false;
    this.context = null;
  }

  private get timers(): TimerHost { return globalThis as unknown as TimerHost; }
  private assertOpen(): void {
    if (this.disposed) throw new ViewerClientError("disposed", "The viewer client has been disposed.");
  }
  private request(method: ViewerProtocol.Method, params: Record<string, unknown>): Promise<ViewerContext | null> {
    this.assertOpen();
    const request_id = this.prefix + ++this.nextRequest;
    return new Promise((resolve, reject) => {
      const timer = this.timers.setTimeout(() => {
        this.pending.delete(request_id);
        reject(new ViewerClientError("timeout", "No confirmed viewer reply arrived before the request timeout."));
      }, this.timeoutMs);
      this.pending.set(request_id, { method, resolve, reject, timer });
      try {
        this.target.postMessage({ channel: ViewerProtocol.CHANNEL, version: ViewerProtocol.VERSION,
          type: "request", request_id, method, params } satisfies ViewerProtocol.Request, this.origin);
      } catch {
        this.pending.delete(request_id);
        this.timers.clearTimeout(timer);
        reject(new ViewerClientError("transport_error", "The viewer request could not be posted."));
      }
    });
  }
  private onMessage = (event: ViewerMessageEvent): void => {
    if (this.disposed || event.source !== this.target || event.origin !== this.origin) return;
    const message = event.data;
    if (!ViewerProtocol.record(message) || message.channel !== ViewerProtocol.CHANNEL) return;
    if (message.type === "context_changed") {
      if (!ViewerProtocol.isContextEvent(message) || !this.remoteSubscriptionPossible) return;
      this.context = message.context;
      this.contextEvents++;
      for (const listener of [...this.listeners]) if (listener.initialized) this.notify(listener, message.context);
      return;
    }
    if (message.type !== "response" || typeof message.request_id !== "string") return;
    const pending = this.pending.get(message.request_id);
    if (!pending) return;
    this.pending.delete(message.request_id);
    this.timers.clearTimeout(pending.timer);
    if (message.version !== ViewerProtocol.VERSION) {
      pending.reject(new ViewerClientError("unsupported_version", "The viewer replied with an unsupported protocol version."));
    } else if (!ViewerProtocol.isResponse(message)
      || message.ok && ((pending.method === "unsubscribe") !== (message.result === null))) {
      pending.reject(new ViewerClientError("invalid_response", "The viewer returned a malformed response."));
    } else if (!message.ok) {
      pending.reject(new ViewerClientError(message.error.code, message.error.message));
    } else {
      pending.resolve(message.result === null ? null : this.copy(message.result));
    }
  };

  private async ensureSubscribed(): Promise<ViewerContext> {
    if (this.unsubscriptionPending) await this.unsubscriptionPending;
    this.assertOpen();
    if (this.subscriptionPending) return this.subscriptionPending;
    if (this.subscribed && this.context) return this.context;
    const before = this.contextEvents;
    this.remoteSubscriptionPossible = true;
    this.subscriptionPending = this.request("subscribe", {}).then((context) => {
      this.assertOpen();
      this.subscribed = true;
      if (this.contextEvents === before || !this.context) this.context = context!;
      return this.context;
    }).catch((error: unknown) => {
      this.subscribed = false;
      if (!this.disposed) this.unsubscribeBestEffort();
      throw error;
    }).finally(() => { this.subscriptionPending = null; });
    return this.subscriptionPending;
  }
  private unsubscribeRemote(): void {
    if (this.disposed || !this.subscribed || this.unsubscriptionPending) return;
    this.subscribed = false;
    this.unsubscriptionPending = this.request("unsubscribe", {}).then(() => {
      this.remoteSubscriptionPossible = false;
      this.context = null;
    }).catch(() => { /* A lost acknowledgement does not imply remote cleanup. */ })
      .finally(() => { this.unsubscriptionPending = null; });
  }
  private unsubscribeBestEffort(): void {
    try {
      this.target.postMessage({ channel: ViewerProtocol.CHANNEL, version: ViewerProtocol.VERSION,
        type: "request", request_id: this.prefix + ++this.nextRequest, method: "unsubscribe", params: {} } satisfies ViewerProtocol.Request, this.origin);
    } catch { /* Closing a page must not prevent local cleanup. */ }
  }
  private notify(listener: Listener, context: ViewerContext): void {
    if (this.disposed || !this.listeners.has(listener)) return;
    try { listener.callback(this.copy(context)); } catch { /* One listener cannot interrupt the others. */ }
  }
  private copy(context: ViewerContext): ViewerContext { return JSON.parse(JSON.stringify(context)) as ViewerContext; }
}
