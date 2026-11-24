import type { ViewerPollingScheduler } from "./controller";
import { parseViewerRoute, viewerEntryUrl, type ViewerRoute } from "./route";

export interface RunHistory {
  kind: "fx-viewer-run-history-v1";
  run_id: string;
  latest_url: string;
  latest_available: boolean;
  selected_attempt: number;
  attempts: { attempt_number: number; url: string; state: string }[];
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function attemptNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

export function parseRunHistory(value: unknown, route: ViewerRoute): RunHistory {
  const invalid = () => new Error("The run history response does not match this viewer address.");
  if (route.kind !== "run" || !route.project_id || !route.entry_id || !record(value)
    || value.kind !== "fx-viewer-run-history-v1" || typeof value.run_id !== "string"
    || !/^[0-9a-f]{64}$/.test(value.run_id) || value.latest_url !== `/p/${route.project_id}/run/${value.run_id}/`
    || typeof value.latest_available !== "boolean"
    || !attemptNumber(value.selected_attempt) || !Array.isArray(value.attempts)) throw invalid();
  const selectedUrl = viewerEntryUrl(route.project_id, "run", route.entry_id);
  const urls = new Set<string>();
  let previous = Infinity, hasSelected = false;
  for (const attempt of value.attempts) {
    if (!record(attempt) || !attemptNumber(attempt.attempt_number) || attempt.attempt_number >= previous
      || typeof attempt.state !== "string" || !attempt.state
      || typeof attempt.url !== "string" || urls.has(attempt.url)) throw invalid();
    const target = parseViewerRoute(attempt.url);
    if (target.project_id !== route.project_id || target.kind !== "run" || !target.entry_id
      || attempt.url !== viewerEntryUrl(target.project_id, "run", target.entry_id)) throw invalid();
    if (attempt.attempt_number === value.selected_attempt) hasSelected = attempt.url === selectedUrl;
    previous = attempt.attempt_number;
    urls.add(attempt.url);
  }
  if (!hasSelected) throw invalid();
  return value as unknown as RunHistory;
}

type ReadHistory = (url: string, signal: AbortSignal) => Promise<unknown>;

async function readHistory(url: string, signal: AbortSignal): Promise<unknown> {
  const response = await fetch(url, { mode: "same-origin", cache: "no-store", signal });
  if (response.status === 404 || response.status === 405) return null;
  if (!response.ok) throw new Error(`Could not refresh run history (HTTP ${response.status}).`);
  return response.json();
}

const pollingScheduler: ViewerPollingScheduler = {
  schedule: (callback, delay) => setTimeout(callback, delay),
  cancel: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

interface RunHistoryState { history: RunHistory | null; error: string | null }

/** Reads the selected attempt's history and polls only while its panel is open. */
export class RunHistoryController {
  private state: RunHistoryState = { history: null, error: null };
  private readonly listeners = new Set<() => void>();
  private readonly requestHistory: ReadHistory;
  private readonly scheduler: ViewerPollingScheduler;
  private active = false;
  private open = false;
  private timer: unknown = null;
  private request: AbortController | null = null;

  constructor(private readonly route: ViewerRoute | null, options: {
    reader?: ReadHistory;
    scheduler?: ViewerPollingScheduler;
  } = {}) {
    this.requestHistory = options.reader ?? readHistory;
    this.scheduler = options.scheduler ?? pollingScheduler;
  }

  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };

  start() {
    if (this.active) return;
    this.active = true;
    void this.refresh();
  }

  dispose() {
    this.active = false;
    this.cancelTimer();
    this.request?.abort();
    this.request = null;
  }

  setOpen(open: boolean) {
    if (this.open === open) return;
    this.open = open;
    this.cancelTimer();
    if (open) void this.refresh();
  }

  private cancelTimer() {
    if (this.timer !== null) this.scheduler.cancel(this.timer);
    this.timer = null;
  }

  private update(next: RunHistoryState) {
    if (next.error === this.state.error && JSON.stringify(next.history) === JSON.stringify(this.state.history)) return;
    this.state = next;
    this.listeners.forEach(listener => listener());
  }

  private async refresh() {
    if (!this.active || this.route?.kind !== "run" || this.request) return;
    const request = new AbortController();
    this.request = request;
    try {
      const value = await this.requestHistory(`${this.route.api_base}/history`, request.signal);
      if (request.signal.aborted) return;
      if (value !== null) {
        this.update({ history: parseRunHistory(value, this.route), error: null });
      } else {
        this.update({ history: this.state.history, error: this.state.history ? "Run history is unavailable." : null });
      }
    } catch (error) {
      if (!request.signal.aborted) this.update({ history: this.state.history,
        error: error instanceof Error ? error.message : "Could not refresh run history." });
    } finally {
      if (!request.signal.aborted) {
        this.request = null;
        if (this.open) this.timer = this.scheduler.schedule(() => { this.timer = null; void this.refresh(); }, 2000);
      }
    }
  }
}
