import { parseGraph, parseLayoutReport, parseViewerRun, type GraphDocument, type LayoutReport, type ViewerRun } from "./index";
import { validateViewerApiBase } from "./route";

/** The event envelope is stable; a consumer may ignore unfamiliar event names or fields. */
export interface RunEvent extends Record<string, unknown> {
  kind: "fx-run-events-v1";
  event: string;
  invocation_id: string;
  plan: string;
  offset_ms: number;
}

export interface RunSnapshot {
  kind: "fx-run-snapshot-v1";
  cursor: string;
  plan: GraphDocument;
  events: RunEvent[];
}

/** The loopback viewer adds a projection and a layout report of exactly the snapshot's event prefix. */
export interface ViewerSnapshot extends RunSnapshot { view: ViewerRun; layout?: LayoutReport; run_active?: boolean }

/** Recorded intent for the latest invocation, independent of owner liveness. */
export function cancellationRequested(events: readonly RunEvent[]): boolean {
  let invocation: string | null = null;
  let requested = false;
  for (const event of events) {
    if (event.event === "run_started") { invocation = event.invocation_id; requested = false; }
    if (event.invocation_id !== invocation) continue;
    if (event.event === "cancel_requested") requested = true;
    if (event.event === "run_finished" || event.event === "run_cancelled") requested = false;
  }
  return requested;
}

export interface RunEventBatch {
  kind: "fx-run-event-batch-v1";
  cursor: string;
  events: RunEvent[];
  has_more: boolean;
}

export class ObservationError extends Error {
  constructor(readonly code: string, message: string, readonly status: number) { super(message); }
  get requiresSnapshot() { return this.status === 409 && ["invalid_cursor", "run_changed"].includes(this.code); }
}

const RUN_ACTIVE_HEADER = "x-fx-run-active";
export type ObservedInvocation = Pick<RunEvent, "plan" | "invocation_id">;

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function cursor(value: unknown): value is string { return typeof value === "string" && value.length > 0; }

function event(value: unknown): value is RunEvent {
  return record(value) && value.kind === "fx-run-events-v1"
    && typeof value.event === "string" && value.event.length > 0
    && typeof value.invocation_id === "string" && value.invocation_id.length > 0
    && typeof value.plan === "string" && /^[a-f0-9]{64}$/.test(value.plan)
    && typeof value.offset_ms === "number" && Number.isSafeInteger(value.offset_ms) && value.offset_ms >= 0;
}

export function parseRunSnapshot(value: unknown): RunSnapshot {
  if (!record(value) || value.kind !== "fx-run-snapshot-v1") throw new Error("This viewer does not support the returned observation format.");
  if (!cursor(value.cursor) || !Array.isArray(value.events) || !value.events.every(event)) throw new Error("The run snapshot is incomplete or malformed.");
  parseGraph(value.plan);
  return value as unknown as RunSnapshot;
}

export function parseViewerSnapshot(value: unknown, apiBase = "/api"): ViewerSnapshot {
  const snapshot = parseRunSnapshot(value);
  const { view, layout } = value as Record<string, unknown>;
  parseViewerRun(view, apiBase);
  if (layout !== undefined) parseLayoutReport(layout);
  return snapshot as ViewerSnapshot;
}

export function parseRunEventBatch(value: unknown): RunEventBatch {
  if (!record(value) || value.kind !== "fx-run-event-batch-v1") throw new Error("This viewer does not support the returned observation format.");
  if (!cursor(value.cursor) || !Array.isArray(value.events) || !value.events.every(event) || typeof value.has_more !== "boolean") throw new Error("The run event batch is incomplete or malformed.");
  return value as unknown as RunEventBatch;
}

async function readResponse(url: string, signal: AbortSignal) {
  const response = await fetch(url, { signal, cache: "no-store" });
  if (!response.ok) {
    const body: unknown = await response.json().catch(() => null);
    if (record(body) && body.kind === "fx-run-observation-error-v1" && typeof body.code === "string" && typeof body.message === "string") {
      throw new ObservationError(body.code, body.message, response.status);
    }
    throw new Error(`The run could not be observed (HTTP ${response.status}).`);
  }
  return response;
}

export interface RunObservationReader {
  snapshot(signal: AbortSignal): Promise<ViewerSnapshot>;
  events(after: string, signal: AbortSignal, invocation?: ObservedInvocation): Promise<RunEventBatch & { run_active?: boolean }>;
}

/** Relative HTTP endpoints keep observation independent of the worker's filesystem. */
export function observationReader(apiBase = "/api"): RunObservationReader {
  validateViewerApiBase(apiBase);
  return {
    async snapshot(signal) {
      const response = await readResponse(`${apiBase}/snapshot`, signal);
      return { ...parseViewerSnapshot(await response.json(), apiBase), run_active: response.headers.get(RUN_ACTIVE_HEADER) === "1" };
    },
    async events(after, signal, invocation) {
      const query = new URLSearchParams({ after, limit: "256" });
      if (invocation) {
        query.set("plan", invocation.plan);
        query.set("invocation_id", invocation.invocation_id);
      }
      const response = await readResponse(`${apiBase}/events?${query}`, signal);
      return { ...parseRunEventBatch(await response.json()), run_active: response.headers.get(RUN_ACTIVE_HEADER) === "1" };
    },
  };
}

export const readObservation: RunObservationReader = observationReader();

/** A saved plan's layout report; a run's arrives with its snapshot. */
export async function readLayout(signal: AbortSignal, apiBase = "/api"): Promise<LayoutReport> {
  const response = await readResponse(`${validateViewerApiBase(apiBase)}/layout`, signal);
  return parseLayoutReport(await response.json());
}
