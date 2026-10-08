import { parseViewerView, type ViewerView } from "./index";
import type { ViewerPollingScheduler } from "./controller";
import { parseViewerRoute, viewerEntryUrl, type ViewerRoute } from "./route";

export interface ServiceEntry {
  id: string;
  kind: "run" | "plan";
  title: string;
  url: string;
  state: string;
  workflow?: { id: string; source: string };
  created_at?: string;
  name?: string;
}

export interface ServiceIndex {
  kind: "fx-service-index-v1";
  project_id: string;
  title: string;
  entries: ServiceEntry[];
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function utcTimestamp(value: unknown): value is string {
  if (typeof value !== "string") return false;
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?Z$/.exec(value);
  if (!match) return false;
  const date = new Date(value);
  return Number.isFinite(date.getTime())
    && [date.getUTCFullYear(), date.getUTCMonth() + 1, date.getUTCDate(), date.getUTCHours(), date.getUTCMinutes(), date.getUTCSeconds()]
      .every((part, index) => part === Number(match[index + 1]));
}

function workflow(value: unknown): value is NonNullable<ServiceEntry["workflow"]> {
  return record(value) && typeof value.id === "string" && value.id.length > 0
    && !/[\x00-\x1f\x7f]/.test(value.id)
    && typeof value.source === "string" && value.source.length > 0
    && !/[\\\x00-\x1f\x7f]/.test(value.source)
    && !/^(?:\/|[A-Za-z]:|file:|[A-Za-z][A-Za-z0-9+.-]*:\/\/)/i.test(value.source);
}

export function parseServiceIndex(value: unknown): ServiceIndex {
  if (!record(value) || value.kind !== "fx-service-index-v1") throw new Error("This viewer does not support the returned project format.");
  if (typeof value.project_id !== "string" || !/^[a-f0-9]{64}$/.test(value.project_id)
    || typeof value.title !== "string" || !Array.isArray(value.entries)) throw new Error("The project response is incomplete or malformed.");
  const entries = new Set<string>();
  for (const entry of value.entries) {
    if (!record(entry) || typeof entry.id !== "string" || !/^[a-f0-9]{64}$/.test(entry.id)
      || !["run", "plan"].includes(entry.kind as string) || typeof entry.title !== "string"
      || typeof entry.state !== "string"
      || (entry.workflow !== undefined && !workflow(entry.workflow))
      || (entry.created_at !== undefined && !utcTimestamp(entry.created_at))
      || (entry.name !== undefined && (entry.kind !== "run" || typeof entry.name !== "string" || !/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(entry.name)))
      || entry.url !== viewerEntryUrl(value.project_id, entry.kind as "run" | "plan", entry.id)) {
      throw new Error("The project response contains an invalid entry.");
    }
    const key = `${entry.kind}:${entry.id}`;
    if (entries.has(key)) throw new Error("The project response contains duplicate entries.");
    entries.add(key);
  }
  return value as unknown as ServiceIndex;
}

export interface ServiceWorkflowGroup {
  key: string;
  title: string;
  workflow?: NonNullable<ServiceEntry["workflow"]>;
  /** Latest recorded run, or the newest saved plan when no run exists. */
  primary: ServiceEntry;
  /** Undated legacy records do not establish which run was created latest. */
  latest_known: boolean;
  show_source: boolean;
  runs: ServiceEntry[];
  plans: ServiceEntry[];
  /** Recorded in-progress evidence, not an assertion that a process is alive. */
  unfinished_runs: ServiceEntry[];
}

function compareText(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

function newestEntry(left: ServiceEntry, right: ServiceEntry): number {
  if (left.created_at !== undefined && right.created_at !== undefined) {
    const [leftSecond, leftFraction = ""] = left.created_at.slice(0, -1).split(".");
    const [rightSecond, rightFraction = ""] = right.created_at.slice(0, -1).split(".");
    const seconds = compareText(rightSecond, leftSecond);
    if (seconds) return seconds;
    const precision = Math.max(leftFraction.length, rightFraction.length);
    const fractions = compareText(rightFraction.padEnd(precision, "0"), leftFraction.padEnd(precision, "0"));
    if (fractions) return fractions;
  } else if (left.created_at !== undefined || right.created_at !== undefined) {
    return left.created_at !== undefined ? -1 : 1;
  }
  return compareText(left.kind, right.kind) || compareText(left.id, right.id);
}

/** Groups recorded workflow identities without deriving identity from titles or plan digests. */
export class ServiceIndexProjection {
  readonly groups: ServiceWorkflowGroup[];

  constructor(catalog: ServiceIndex) {
    const groups = new Map<string, ServiceEntry[]>();
    for (const entry of catalog.entries) {
      const key = JSON.stringify(entry.workflow
        ? ["workflow", catalog.project_id, entry.workflow.id, entry.workflow.source]
        : ["entry", catalog.project_id, entry.kind, entry.id]);
      const entries = groups.get(key) ?? [];
      entries.push(entry);
      groups.set(key, entries);
    }
    this.groups = [...groups].map(([key, entries]) => {
      const runs = entries.filter((entry) => entry.kind === "run").sort(newestEntry);
      const plans = entries.filter((entry) => entry.kind === "plan").sort(newestEntry);
      const primary = runs[0] ?? plans[0];
      return {
        key, title: primary.title, workflow: primary.workflow, primary,
        latest_known: (runs.length ? runs : plans).every((entry) => entry.created_at !== undefined),
        show_source: false, runs, plans,
        unfinished_runs: runs.filter((entry) => entry !== primary && ["running", "unfinished"].includes(entry.state)),
      };
    });
    const titles = new Map<string, number>(), workflowIds = new Map<string, number>();
    for (const group of this.groups) {
      titles.set(group.title, (titles.get(group.title) ?? 0) + 1);
      if (group.workflow) workflowIds.set(group.workflow.id, (workflowIds.get(group.workflow.id) ?? 0) + 1);
    }
    for (const group of this.groups) {
      group.show_source = group.workflow !== undefined
        && ((titles.get(group.title) ?? 0) > 1 || (workflowIds.get(group.workflow.id) ?? 0) > 1);
    }
    this.groups.sort((left, right) => newestEntry(left.primary, right.primary)
      || compareText(left.title, right.title) || compareText(left.key, right.key));
  }
}

export type ViewerEntry = ViewerView | ServiceIndex;

export function parseViewerEntry(value: unknown, route: ViewerRoute): ViewerEntry {
  if (record(value) && value.kind === "fx-service-index-v1") {
    if (route.kind !== "root") throw new Error("The server returned a project for a workflow address.");
    return parseServiceIndex(value);
  }
  const view = parseViewerView(value, route.api_base);
  if ((route.kind === "plan" && view.kind !== "fx-graph-v1")
    || (route.kind === "run" && view.kind !== "fx-viewer-run-v1")) throw new Error("The server returned a different kind of workflow entry.");
  return view;
}

async function readEntry(url: string, signal: AbortSignal): Promise<unknown> {
  const response = await fetch(url, { signal, cache: "no-store" });
  if (!response.ok) throw new Error(`FX could not be read (HTTP ${response.status}).`);
  return response.json();
}

export interface ViewerEntryState {
  kind: "loading" | "viewer" | "catalog";
  catalog: ServiceIndex | null;
  loading: boolean;
  error: string | null;
}

export interface ViewerEntryOptions {
  reader?: (url: string, signal: AbortSignal) => Promise<unknown>;
  scheduler?: ViewerPollingScheduler;
}

const scheduler: ViewerPollingScheduler = {
  schedule: (callback, delay) => setTimeout(callback, delay),
  cancel: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/** Resolves standalone/project pages and follows the project index without overlapping reads. */
export class ViewerEntryController {
  readonly route: ViewerRoute | null;
  private state: ViewerEntryState = { kind: "loading", catalog: null, loading: true, error: null };
  private listeners = new Set<() => void>();
  private readonly reader: NonNullable<ViewerEntryOptions["reader"]>;
  private readonly scheduler: ViewerPollingScheduler;
  private request: AbortController | null = null;
  private timer: unknown = null;
  private active = false;
  private failures = 0;
  private routeError: string | null = null;

  constructor(pathname: string, options: ViewerEntryOptions = {}) {
    this.reader = options.reader ?? readEntry;
    this.scheduler = options.scheduler ?? scheduler;
    try { this.route = parseViewerRoute(pathname); }
    catch (error) { this.route = null; this.routeError = (error as Error).message; }
  }

  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(next: Partial<ViewerEntryState>) {
    this.state = { ...this.state, ...next };
    this.listeners.forEach((listener) => listener());
  }
  private cancelTimer() {
    if (this.timer !== null) this.scheduler.cancel(this.timer);
    this.timer = null;
  }
  private schedule() {
    this.cancelTimer();
    if (!this.active || this.state.kind !== "catalog") return;
    this.timer = this.scheduler.schedule(() => { this.timer = null; void this.load(false); }, 2000 * 2 ** Math.min(this.failures, 2));
  }
  private async load(manual: boolean) {
    this.cancelTimer();
    this.request?.abort();
    if (!this.route) { this.update({ loading: false, error: this.routeError }); return; }
    const request = new AbortController();
    this.request = request;
    if (manual) this.update({ loading: true });
    try {
      const catalogRead = this.state.kind === "catalog";
      const value = await this.reader(catalogRead ? "/api/catalog" : `${this.route.api_base}/view`, request.signal);
      if (request.signal.aborted) return;
      const entry = catalogRead ? parseServiceIndex(value) : parseViewerEntry(value, this.route);
      if (entry.kind === "fx-service-index-v1") {
        if (this.state.catalog && this.state.catalog.project_id !== entry.project_id) throw new Error("A different project now owns this address. Reload to open that project.");
        // Avoid replacing unchanged list data during background polls.
        const catalog = JSON.stringify(this.state.catalog) === JSON.stringify(entry) ? this.state.catalog : entry;
        if (catalog !== this.state.catalog || this.state.error !== null) this.update({ kind: "catalog", catalog, error: null });
      } else this.update({ kind: "viewer", error: null });
      this.failures = 0;
    } catch (error) {
      if (!request.signal.aborted) {
        this.failures++;
        this.update({ error: error instanceof Error ? error.message : "The FX project could not be loaded." });
      }
    } finally {
      if (!request.signal.aborted) {
        if (this.state.loading) this.update({ loading: false });
        this.schedule();
      }
    }
  }
  refresh = async () => { this.active = true; await this.load(true); };
  dispose() { this.active = false; this.cancelTimer(); this.request?.abort(); }
}
