import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { CircleCheck, Clock, X } from "lucide-react";
import { ReviewValue, WaitingAnswerSummary } from "./waiting-answer-summary";
import { answerSchema, countdownLabel, encodeAnswer, initialAnswer, parseDraft, WaitingReviewController, type ViewerRun, type Artifact, type WaitingRequest } from "@grida/fx-web";
import { WaitingAnswerFields } from "./waiting-answer-fields";

function localTime(value: string) {
  return <time dateTime={value} title={value}>{new Date(value).toLocaleString()}</time>;
}

function RequestCountdown({ deadline }: { deadline: string }) {
  const [now, setNow] = useState(Date.now);
  const deadlineMs = Date.parse(deadline);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  return <p className="fx-review-meta">
    <Clock size={12} aria-hidden="true" />
    <time role="timer" dateTime={deadline} title={new Date(deadlineMs).toLocaleString()}>
      {countdownLabel(deadlineMs, now)}
    </time>
  </p>;
}

function RequestForm({ request, artifacts, controller, formState }: {
  request: WaitingRequest; artifacts: Map<string, Artifact>; controller: WaitingReviewController;
  formState: ReturnType<WaitingReviewController["getSnapshot"]>;
}) {
  const { drafts, formDrafts, notices, submitting } = formState;
  const state = request.state;
  const formErrorId = useId();
  const [fieldError, setFieldError] = useState<{ control: HTMLElement | null; message: string } | null>(null);
  const card = useRef<HTMLDivElement>(null);
  const feedback = useRef<HTMLParagraphElement>(null);
  const resolution = useRef<HTMLDivElement>(null);
  const wasPending = useRef(state.status === "pending");
  const text = drafts.get(request.id) ?? "";
  const draft = parseDraft(text);
  const key = request.choice_field && draft && typeof draft === "object"
    ? (draft as Record<string, unknown>)[request.choice_field] : draft;
  const choices = request.choices ? Object.entries(request.choices) : [];
  const schema = typeof request.schema === "object" ? request.schema : {};
  const properties = answerSchema(schema.properties);
  const namedFields = typeof properties === "object" ? Object.entries(properties) : [];
  const choiceField = choices.length ? request.choice_field ?? undefined : undefined;
  const extraFields = namedFields.filter(([name]) => name !== choiceField);
  const choiceOnly = !choiceField || (schema.additionalProperties === false
    && !schema.patternProperties && extraFields.length === 0);
  const showForm = choices.length === 0 || !choiceOnly;
  const formSchema = choiceField && request.schema !== false ? {
    ...schema, type: "object", properties: Object.fromEntries(extraFields),
    required: Array.isArray(schema.required) ? schema.required.filter(name => name !== choiceField) : [],
  } : request.schema;
  const formDraft = formDrafts.get(request.id) ?? initialAnswer(typeof formSchema === "object" ? formSchema : {});
  const answer = !showForm ? text || undefined : choiceField
    ? formDraft?.kind === "object" && typeof key === "string"
      ? encodeAnswer({ ...formDraft, fields: [...formDraft.fields, { name: choiceField, value: { kind: "string", text: key } }] })
      : undefined
    : encodeAnswer(formDraft);
  const busy = submitting.has(request.id);
  const notice = notices.get(request.id);
  const errorMessage = fieldError?.message || notice;
  useLayoutEffect(() => {
    if (!fieldError) return;
    const { control } = fieldError;
    if (!control) {
      feedback.current?.focus();
      return;
    }
    const help = control.getAttribute("aria-describedby");
    control.setAttribute("aria-invalid", "true");
    control.setAttribute("aria-describedby", help ? `${help} ${formErrorId}` : formErrorId);
    control.focus();
    return () => {
      control.removeAttribute("aria-invalid");
      if (help) control.setAttribute("aria-describedby", help);
      else control.removeAttribute("aria-describedby");
    };
  }, [fieldError, formErrorId]);
  useEffect(() => {
    const active = document.activeElement;
    if (active === document.body || card.current?.contains(active)) {
      if (state.status !== "pending" && wasPending.current) resolution.current?.focus();
      else if (state.status === "pending" && notice) feedback.current?.focus();
    }
    wasPending.current = state.status === "pending";
  }, [state.status, notice]);
  return <div ref={card} className="fx-review-card">
    <p className="fx-review-step">{request.step}</p>
    {state.status === "pending" && (request.deadline
      ? <RequestCountdown deadline={request.deadline} />
      : <p className="fx-review-meta">Awaiting answer</p>)}
    {state.status === "pending" && request.show != null && <div className="fx-review-media">
      <ReviewValue value={request.show} artifacts={artifacts} encoded />
    </div>}
    {state.status === "pending" ? <form noValidate aria-busy={busy || undefined} onChange={() => setFieldError(null)} onSubmit={event => {
      event.preventDefault();
      if (busy || request.schema === false) return;
      const control = event.currentTarget.querySelector<HTMLElement>("[data-answer-error]");
      if (control || answer === undefined) {
        setFieldError({ control, message: control?.dataset.answerError
          ?? "Check that every field has a value and that field names are unique." });
        return;
      }
      setFieldError(null);
      void controller.submit(request.id, answer);
    }}>
      <fieldset disabled={busy}>
        {choices.length > 0 && <fieldset><legend>Choose a candidate<span className="fx-answer-required" aria-hidden="true"> (required)</span></legend>
          <div className="fx-review-choices">
          {choices.map(([label, value]) => <div key={label} className="fx-review-choice" data-selected={key === label}>
            <label className="fx-answer-choice"><input type="radio" name={request.id} value={label} checked={key === label} required
              data-answer-error={typeof key !== "string" ? "Choose a candidate." : undefined}
              onChange={() => controller.choose(request, label)} />{label}</label>
            <ReviewValue value={value} artifacts={artifacts} encoded />
          </div>)}
          </div>
        </fieldset>}
        {showForm && <WaitingAnswerFields label={choiceField ? "Additional details" : "Answer"}
          schema={formSchema} draft={formDraft} required={!choiceField} onChange={value => {
            setFieldError(null);
            controller.editForm(request.id, value);
          }} />}
      </fieldset>
      <p className="fx-review-error" id={formErrorId} role="alert" tabIndex={-1} ref={feedback}>{errorMessage}</p>
      <button type="submit" className="fx-action fx-review-submit"
        disabled={request.schema === false} aria-disabled={busy || undefined}
        aria-describedby={errorMessage ? formErrorId : undefined}>
        {busy ? "Submitting…" : "Submit answer"}
      </button>
    </form> : <div ref={resolution} tabIndex={-1} className="fx-review-resolution" role="status">
      {state.status === "answered" ? <><p><CircleCheck size={14} aria-hidden="true" /> Answered via {state.channel} · {localTime(state.at)}</p>
        <WaitingAnswerSummary request={request} artifacts={artifacts} /></> : <p>Expired {localTime(state.at)}</p>}
    </div>}
    {state.status !== "pending" && notice && <p role="alert" className="fx-review-error">{notice}</p>}
  </div>;
}

/** Mounted only by the live run application. Portable export viewers have no response transport. */
export function WaitingReviews({ run, controller, onClose }: { run: ViewerRun; controller: WaitingReviewController; onClose: () => void }) {
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  const { requests, error, openStep } = state;
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { if (openStep !== null) heading.current?.focus({ preventScroll: true }); }, [openStep]);
  const artifacts = useMemo(() => new Map(run.artifacts.map(item => [item.digest, item])), [run.artifacts]);
  if (openStep === null) return null;
  const request = requests.find(item => item.step === openStep);
  const title = request?.message ?? run.nodes.find(node => node.id === openStep)?.title ?? openStep;
  return <aside className="fx-details fx-review-panel" id="fx-review-panel" aria-label="Workflow requests" data-fx-owns-keys
    onKeyDown={event => {
      if (event.key === "Escape" && !event.defaultPrevented && !event.currentTarget.ownerDocument.fullscreenElement) {
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }
    }}>
    <header className="fx-review-heading">
      <h2 ref={heading} tabIndex={-1}>{title}</h2>
      <button type="button" className="fx-review-close" aria-label="Close review" onClick={onClose}>
        <X size={15} aria-hidden="true" />
      </button>
    </header>
    <div className="fx-review-list">
      {error && <p role="alert" className="fx-review-error">{error}</p>}
      {request ? <RequestForm key={request.id} request={request} artifacts={artifacts} controller={controller} formState={state} />
        : !error && <p role="status" className="fx-review-card">Loading review…</p>}
    </div>
  </aside>;
}
