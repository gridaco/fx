import { validateViewerApiBase } from "./route";
import type { AnswerDraft } from "./answer-draft";
import type { CanvasGraph, CanvasStep } from "./graph";

export type RequestResolution =
  | { status: "pending" }
  | { status: "answered"; answer: unknown; channel: string; at: string }
  | { status: "expired"; at: string };

export interface WaitingRequest {
  id: string;
  step: string;
  message?: string;
  schema: boolean | Record<string, unknown>;
  show?: unknown;
  choices?: Record<string, unknown> | null;
  choice_field?: string | null;
  deadline?: string | null;
  state: RequestResolution;
}

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function parseWaitingRequest(value: unknown): WaitingRequest {
  if (object(value) && typeof value.id === "string" && /^[0-9a-f]{64}$/.test(value.id)
      && typeof value.step === "string" && (value.message === undefined || typeof value.message === "string")
      && (typeof value.schema === "boolean" || object(value.schema))
      && (value.choices == null || object(value.choices))
      && (value.choice_field == null || typeof value.choice_field === "string")
      && (value.deadline == null || typeof value.deadline === "string" && Number.isFinite(Date.parse(value.deadline)))
      && object(value.state)) {
    const state = value.state;
    if (state.status === "pending" || (typeof state.at === "string"
        && (state.status === "expired" || state.status === "answered" && "answer" in state && typeof state.channel === "string"))) {
      return value as unknown as WaitingRequest;
    }
  }
  throw new Error("The viewer received an invalid workflow request.");
}

export interface ReviewState {
  requests: WaitingRequest[];
  openStep: string | null;
  error: string | null;
  submitting: ReadonlySet<string>;
  drafts: ReadonlyMap<string, string>;
  formDrafts: ReadonlyMap<string, AnswerDraft>;
  notices: ReadonlyMap<string, string>;
}

type ReviewCard = { message?: string; status: RequestResolution["status"]; deadline?: string };

/** Keep elapsed deadlines at zero until an authoritative request update arrives. */
export function countdownLabel(deadlineMs: number, now = Date.now()): string {
  const remainingSeconds = Math.max(0, Math.ceil((deadlineMs - now) / 1000));
  const hours = Math.floor(remainingSeconds / 3600);
  const minutes = Math.floor(remainingSeconds / 60) % 60;
  const seconds = String(remainingSeconds % 60).padStart(2, "0");
  const remaining = hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}`
    : `${minutes}:${seconds}`;
  return `${remaining} left`;
}

/** Add live actions to the exact recorded step. A plan or exported graph has no request controller. */
export function withReviewCards(graph: CanvasGraph, reviews: ReadonlyMap<string, ReviewCard>): CanvasGraph {
  let changed = false;
  const nodes = graph.nodes.map<CanvasStep>(node => {
    if (node.kind || node.pending) return node;
    const request = reviews.get(node.source_id);
    if (!request && node.state !== "awaiting") return node;
    const review_action = request?.status === "answered" ? "view_answer"
      : request?.status === "expired" ? "view_outcome" : "review";
    changed = true;
    return {
      ...node, review_action,
      ...(request?.message ? { review_prompt: request.message } : {}),
      ...(request?.status === "pending" && request.deadline ? { review_deadline: request.deadline } : {}),
    };
  });
  return changed ? { ...graph, nodes } : graph;
}

/** Native local run viewer only. All answer validation and arbitration belong to FX.
 * Polling continues when execution has exited so a second client can resolve a parked run. */
export class WaitingReviewController {
  private readonly base: string;
  private readonly listeners = new Set<() => void>();
  private state: ReviewState = { requests: [], openStep: null, error: null, submitting: new Set(), drafts: new Map(), formDrafts: new Map(), notices: new Map() };
  private cards = new Map<string, ReviewCard>();
  private session: AbortController | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(apiBase: string, private readonly request: typeof fetch = globalThis.fetch.bind(globalThis)) {
    this.base = validateViewerApiBase(apiBase);
  }

  readonly getSnapshot = (): ReviewState => this.state;
  readonly getCards = (): ReadonlyMap<string, ReviewCard> => this.cards;
  readonly getPendingCount = (): number => {
    let count = 0;
    for (const card of this.cards.values()) {
      if (card.status === "pending") count++;
    }
    return count;
  };
  readonly getOpenStep = (): string | null => this.state.openStep;
  readonly subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  private update(state: Partial<ReviewState>) {
    if (state.requests) {
      const next = new Map<string, ReviewCard>();
      for (const request of state.requests) {
        next.set(request.step, {
          message: request.message,
          status: request.state.status,
          deadline: request.state.status === "pending" ? request.deadline ?? undefined : undefined,
        });
      }
      // Repeat polls and form edits must leave camera, deck and keyboard focus in place.
      if (next.size !== this.cards.size || [...next].some(([step, card]) => {
        const previous = this.cards.get(step);
        return previous?.message !== card.message || previous?.status !== card.status || previous?.deadline !== card.deadline;
      })) this.cards = next;
    }
    this.state = { ...this.state, ...state };
    for (const listener of this.listeners) listener();
  }

  readonly open = (step: string): void => {
    if (this.state.openStep !== step) this.update({ openStep: step });
  };

  readonly close = (): void => {
    if (this.state.openStep !== null) this.update({ openStep: null });
  };

  start() {
    if (this.session) return;
    this.session = new AbortController();
    void this.poll(this.session.signal);
  }

  stop() {
    this.session?.abort();
    this.session = null;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.update({ submitting: new Set() });
  }

  private async poll(signal: AbortSignal) {
    const before = this.state.requests;
    try {
      const response = await this.request(`${this.base}/requests`, { signal, cache: "no-store", mode: "same-origin" });
      if (!response.ok) throw new Error(`Could not read workflow requests (HTTP ${response.status}).`);
      const body: unknown = await response.json();
      if (!object(body) || !Array.isArray(body.requests)) throw new Error("The workflow request list is invalid.");
      const requests = body.requests.map(parseWaitingRequest);
      if (signal.aborted) return;
      // Only a POST receipt observed during this read can supersede its snapshot. An expiry
      // merely projected by an earlier GET can still become pending if the engine clock moves.
      if (before !== this.state.requests) {
        const previous = new Set(before);
        const receipts = new Map(this.state.requests.filter(r => !previous.has(r) && r.state.status !== "pending").map(r => [r.id, r]));
        for (const [index, item] of requests.entries()) requests[index] = receipts.get(item.id) ?? item;
      }
      this.update({ error: null, requests });
    } catch (error) {
      if (signal.aborted) return;
      this.update({ error: error instanceof Error ? error.message : "Could not read workflow requests." });
    }
    if (!signal.aborted) this.timer = setTimeout(() => void this.poll(signal), this.state.error ? 4000 : 1200);
  }

  edit(id: string, text: string) {
    const notices = new Map(this.state.notices);
    notices.delete(id);
    this.update({ drafts: new Map(this.state.drafts).set(id, text), notices });
  }

  editForm(id: string, draft: AnswerDraft | undefined) {
    const notices = new Map(this.state.notices);
    notices.delete(id);
    const formDrafts = new Map(this.state.formDrafts);
    if (draft === undefined) formDrafts.delete(id);
    else formDrafts.set(id, draft);
    this.update({ formDrafts, notices });
  }

  choose(request: WaitingRequest, key: string) {
    if (!request.choices || !Object.hasOwn(request.choices, key)) return;
    const field = request.choice_field;
    if (!field) return this.edit(request.id, JSON.stringify(key));
    const previous = parseDraft(this.state.drafts.get(request.id));
    this.edit(request.id, JSON.stringify({ ...(object(previous) ? previous : {}), [field]: key }, null, 2));
  }

  async submit(id: string, text = this.state.drafts.get(id)) {
    const signal = this.session?.signal;
    if (!signal || signal.aborted || this.state.submitting.has(id)
      || !this.state.requests.some(r => r.id === id && r.state.status === "pending")) return;
    const answer = parseDraft(text);
    if (answer === undefined) {
      this.update({ notices: new Map(this.state.notices).set(id, "Complete your answer before submitting.") });
      return;
    }
    this.update({ submitting: new Set(this.state.submitting).add(id) });
    try {
      const response = await this.request(`${this.base}/requests/${encodeURIComponent(id)}/respond`, {
        method: "POST", mode: "same-origin", cache: "no-store", signal,
        headers: { "Content-Type": "application/json", "X-FX-Action": "respond" },
        // Retain the authored numeric spelling and duplicate keys for FX's strict JSON reader.
        body: `{"answer":${text}}`,
      });
      const body: unknown = await response.json();
      if (signal.aborted) return;
      const winner = response.ok ? body : object(body) ? body.request : null;
      if (winner != null) {
        const request = parseWaitingRequest(winner);
        if (request.id !== id) throw new Error("The response identified a different workflow request.");
        this.update({ requests: this.state.requests.map(r => r.id === id ? request : r) });
      }
      if (!response.ok) {
        if (object(body) && body.code === "invalid_answer") {
          throw new Error("Check the fields in your answer and try again.");
        }
        throw new Error(object(body) && typeof body.error === "string"
          ? body.error : `Could not record the answer (HTTP ${response.status}).`);
      }
      const notices = new Map(this.state.notices);
      notices.delete(id);
      this.update({ notices });
    } catch (error) {
      if (!signal.aborted) this.update({ notices: new Map(this.state.notices).set(id,
        error instanceof Error ? error.message : "Could not confirm the answer. Try submitting it again.") });
    } finally {
      if (!signal.aborted) {
        const submitting = new Set(this.state.submitting);
        submitting.delete(id);
        this.update({ submitting });
      }
    }
  }
}

/** Undefined distinguishes an empty/invalid editor from a valid JSON null, false or zero. */
export function parseDraft(text: string | undefined): unknown {
  if (!text) return undefined;
  try { return JSON.parse(text); } catch { return undefined; }
}
