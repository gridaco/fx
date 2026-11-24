import type { ViewerPollingScheduler } from "./controller";
import { parseViewerRoute, viewerEntryUrl, type ViewerRoute } from "./route";

export const runActions = ["pause", "resume", "rerun", "cancel"] as const;
export type RunAction = typeof runActions[number];

export function requiresRunConfirmation(action: RunAction): boolean {
  return action === "rerun" || action === "cancel";
}

export interface RunControl {
  kind: "fx-viewer-run-control-v1";
  invocation_id: string | null;
  attempt_number: number;
  actions: Record<RunAction, boolean>;
}

export interface RunActionReceipt {
  kind: "fx-viewer-run-action-v1";
  outcome: "accepted" | "refused" | "unknown";
  message: string;
  code?: string;
  url?: string;
  cli_command?: string;
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function parseRunControl(value: unknown): RunControl {
  if (record(value) && value.kind === "fx-viewer-run-control-v1"
    && typeof value.attempt_number === "number" && Number.isSafeInteger(value.attempt_number) && value.attempt_number > 0
    && (value.invocation_id === null || typeof value.invocation_id === "string" && /^[A-Za-z0-9_-]{1,256}$/.test(value.invocation_id))
    && record(value.actions)) {
    const actions = value.actions;
    if (runActions.every(action => typeof actions[action] === "boolean" && (value.invocation_id !== null || !actions[action]))) {
      return value as unknown as RunControl;
    }
  }
  throw new Error("FX returned invalid run controls.");
}

export function parseRunActionReceipt(value: unknown, action: RunAction, route: ViewerRoute): RunActionReceipt {
  const invalid = () => new Error("FX returned an invalid run action response.");
  if (!record(value) || value.kind !== "fx-viewer-run-action-v1"
    || (value.outcome !== "accepted" && value.outcome !== "refused" && value.outcome !== "unknown")
    || typeof value.message !== "string" || value.message.length === 0 || value.message.length > 4096
    || (value.code !== undefined && (typeof value.code !== "string" || !/^[a-z][a-z0-9_]*$/.test(value.code)))
    || (value.cli_command !== undefined && (typeof value.cli_command !== "string" || value.cli_command.length === 0 || value.cli_command.length > 4096))) throw invalid();
  if (value.outcome === "accepted" && (action === "rerun" || action === "resume") && value.url === undefined) throw invalid();
  if (value.url !== undefined) {
    if (typeof value.url !== "string" || value.outcome !== "accepted" || (action !== "rerun" && action !== "resume")) throw invalid();
    const target = parseViewerRoute(value.url);
    if (target.kind !== "run" || target.project_id !== route.project_id || !target.project_id || !target.entry_id
      || value.url !== viewerEntryUrl(target.project_id, "run", target.entry_id)
      || (action === "resume") !== (target.entry_id === route.entry_id)) throw invalid();
  }
  return value as unknown as RunActionReceipt;
}

interface ActionSelection {
  action: RunAction;
  attempt_number: number;
  invocation_id: string;
  request_id?: string;
}

export interface RunActionsState {
  control: RunControl | null;
  error: string | null;
  selection: ActionSelection | null;
  submitting: boolean;
  checking: boolean;
  rerunUnconfirmed: boolean;
  receipt: (RunActionReceipt & { action: RunAction }) | null;
}

interface Options {
  request?: typeof fetch;
  scheduler?: ViewerPollingScheduler;
  requestId?: () => string;
  onAccepted?: (action: RunAction, receipt: RunActionReceipt) => void;
}

const scheduler: ViewerPollingScheduler = {
  schedule: (callback, delay) => setTimeout(callback, delay),
  cancel: handle => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/** Keeps the invocation selected by the user fixed through confirmation and transport retries. */
export class RunActionsController {
  private state: RunActionsState = { control: null, error: null, selection: null, submitting: false, checking: false, rerunUnconfirmed: false, receipt: null };
  private readonly listeners = new Set<() => void>();
  private readonly request: typeof fetch;
  private readonly scheduler: ViewerPollingScheduler;
  private readonly requestId: () => string;
  private readonly base: string | undefined;
  private active = false;
  private open = false;
  private timer: unknown = null;
  private read: AbortController | null = null;
  private unconfirmedRerun: { selection: ActionSelection; receipt: NonNullable<RunActionsState["receipt"]> } | null = null;

  constructor(private readonly route: ViewerRoute | null, private readonly options: Options = {}) {
    this.base = route?.kind === "run" && route.project_id ? `${route.api_base}/control` : undefined;
    this.request = options.request ?? globalThis.fetch.bind(globalThis);
    this.scheduler = options.scheduler ?? scheduler;
    this.requestId = options.requestId ?? (() => globalThis.crypto.randomUUID());
  }

  readonly getSnapshot = () => this.state;
  readonly subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  start() {
    if (this.active || !this.base) return;
    this.active = true;
    void this.refresh();
  }

  dispose() {
    this.active = false;
    this.cancelTimer();
    this.read?.abort();
    this.read = null;
  }

  setOpen(open: boolean) {
    if (this.open === open) return;
    this.open = open;
    this.cancelTimer();
    if (open) {
      this.read?.abort();
      this.read = null;
      this.update({ checking: true });
      void this.refresh();
    } else if (this.state.selection && !this.state.submitting && !this.state.receipt) this.back();
  }

  readonly choose = (action: RunAction, observedInvocation: string | null): void => {
    const { control, checking, error, selection, submitting } = this.state;
    if (!this.active || checking || selection || submitting) return;
    if (action === "rerun" && this.unconfirmedRerun) {
      this.update(this.unconfirmedRerun);
      return;
    }
    if (error || !observedInvocation
      || control?.invocation_id !== observedInvocation || !control.actions[action]) return;
    const { attempt_number, invocation_id } = control;
    const request_id = action === "resume" || action === "rerun" ? this.requestId() : undefined;
    this.update({ selection: { action, invocation_id, attempt_number, request_id }, receipt: null });
    if (!requiresRunConfirmation(action)) void this.submit(observedInvocation);
  };

  readonly back = (): void => {
    if (!this.state.submitting) this.update({ selection: null, receipt: null });
  };

  readonly submit = async (observedInvocation: string | null): Promise<void> => {
    const { base, route } = this;
    const { control, error, selection, receipt, checking, submitting } = this.state;
    if (!this.active || !base || !route || !selection || checking || submitting || receipt?.outcome === "refused") return;
    // An earlier response can be lost after acceptance. Retry that request against its original target.
    if (receipt?.outcome !== "unknown" && (error || !currentRunAction(control, selection, observedInvocation))) return;
    const { action, invocation_id, request_id } = selection;
    this.update({ submitting: true, receipt: null });
    let result: RunActionReceipt;
    try {
      const response = await this.request(`${base}/${action}`, {
        method: "POST", mode: "same-origin", redirect: "error", cache: "no-store",
        headers: { "Content-Type": "application/json", "X-FX-Action": action },
        body: JSON.stringify({ invocation_id, request_id }),
      });
      result = parseRunActionReceipt(await response.json(), action, route);
      if (!response.ok && result.outcome === "accepted") throw new Error("FX did not confirm the action.");
    } catch {
      result = { kind: "fx-viewer-run-action-v1", outcome: "unknown", message: "FX couldn’t tell if the request went through. Retry to check the same request." };
    }
    this.cancelTimer();
    this.read?.abort();
    this.read = null;
    const keepSelection = result.outcome === "unknown" || result.outcome === "refused" && requiresRunConfirmation(action);
    const response = { ...result, action };
    if (action === "rerun") this.unconfirmedRerun = result.outcome === "unknown" ? { selection, receipt: response } : null;
    this.update({ selection: keepSelection ? selection : null, submitting: false, receipt: response, checking: true, rerunUnconfirmed: this.unconfirmedRerun !== null });
    if (this.active) {
      if (result.outcome === "accepted") this.options.onAccepted?.(action, result);
      await this.refresh();
    }
  };

  private update(update: Partial<RunActionsState>) {
    this.state = { ...this.state, ...update };
    this.listeners.forEach(listener => listener());
  }

  private cancelTimer() {
    if (this.timer !== null) this.scheduler.cancel(this.timer);
    this.timer = null;
  }

  private async refresh() {
    if (!this.active || !this.base || this.read) return;
    const read = new AbortController();
    this.read = read;
    try {
      const response = await this.request(this.base, {
        mode: "same-origin", redirect: "error", cache: "no-store", signal: read.signal,
      });
      const supported = response.status !== 404 && response.status !== 405;
      if (supported && !response.ok) throw new Error(`Could not read run actions (HTTP ${response.status}).`);
      const control = supported ? parseRunControl(await response.json()) : null;
      if (read.signal.aborted) return;
      const error = !control && this.state.control ? "Run actions are unavailable." : null;
      if (error !== this.state.error || JSON.stringify(control) !== JSON.stringify(this.state.control) || this.state.checking) {
        this.update({ control: control ?? this.state.control, error, checking: false });
      }
    } catch (error) {
      if (!read.signal.aborted) this.update({ error: "Couldn’t check the run’s status. FX will try again while this panel is open.", checking: false });
    } finally {
      if (!read.signal.aborted) {
        this.read = null;
        if (this.open) this.timer = this.scheduler.schedule(() => {
          this.timer = null;
          void this.refresh();
        }, this.state.error ? 4000 : 1600);
      }
    }
  }
}

export function currentRunAction(control: RunControl | null, selection: ActionSelection, observedInvocation: string | null): boolean {
  return observedInvocation === selection.invocation_id && control?.invocation_id === selection.invocation_id
    && control.attempt_number === selection.attempt_number && control.actions[selection.action];
}
