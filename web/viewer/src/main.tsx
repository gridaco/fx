import { StrictMode, useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { flushSync } from "react-dom";
import { ArrowLeft, ArrowUpRight, Check, ChevronRight, Copy, Download, Ellipsis, FileText, History, List, RefreshCw, Square, Workflow, X } from "lucide-react";
import { ExternalFileController, FileInventoryController, OutputArchiveController, RunActionsController, RunHistoryController, ServiceIndexProjection, ViewerController, ViewerPage, ViewerPresentation, ViewerPresentationError, WaitingReviewController, elapsedLabel, outputFiles, ownsKeys, priceLabel, readFileInventory, skipExplanation, skipSourceScopes, ViewerEntryController, type Artifact, type CanvasPreview, type CanvasStep, type GraphDocument, type GraphInstance, type PendingRepeat, type RunHistory, type ServiceEntry, type ServiceIndex, type ViewerRun, type WorkflowNote, type ValueView, valueArtifacts, valueInspectionData, valueKindLabel, valueStateLabel, withReviewCards } from "@grida/fx-web";
import { ExternalFileLink, NoteContent, StateIcon, StateLabel, Status, Value, Values, WorkflowCanvas, money, type WorkflowCanvasHandle } from "@grida/fx-react";
import { RunFavicon } from "./run-favicon";
import { RunActionsPanel } from "./run-actions";
import { WaitingReviews } from "./waiting-reviews";
import "./style.css";

const entryController = new ViewerEntryController(window.location.pathname);
const apiBase = entryController.route?.api_base ?? "/api";
const controller = new ViewerController(undefined, { apiBase });
const presentation = new ViewerPresentation({
  getSnapshot: controller.getSnapshot,
  restore: (scope, selected) => controller.restore(scope, selected),
  reference: () => {
    const route = entryController.route;
    if (!route?.project_id || !route.entry_id || route.kind === "root") return undefined;
    return { kind: "service_entry", project_id: route.project_id, entry_kind: route.kind, entry_id: route.entry_id };
  },
});
const externalFiles = new ExternalFileController(presentation.getContext);
const fileInventory = new FileInventoryController((signal) => readFileInventory(signal, apiBase));
let filesAvailable = false;
const stopFiles = externalFiles.subscribe(() => {
  const available = externalFiles.getSnapshot().available;
  if (available === filesAvailable) return;
  filesAvailable = available;
  if (available && controller.getSnapshot().view) void fileInventory.refresh();
  else if (!available) fileInventory.clear();
});
let previousView = controller.getSnapshot().view;
const stopInventory = controller.subscribe(() => {
  const view = controller.getSnapshot().view;
  if (view === previousView) return;
  previousView = view;
  if (filesAvailable && view) void fileInventory.refresh();
});
const page = new ViewerPage(window, {
  getContext: () => {
    const entry = entryController.getSnapshot();
    if (entry.kind === "catalog") return {
      subject: { kind: "catalog" }, scope_id: null, selection: null, sampled_at: Date.now(),
      ...(entry.error ? { retained: true as const } : {}),
    };
    if (entry.kind !== "viewer") throw new ViewerPresentationError("not_ready", "No viewer evidence is loaded.");
    return presentation.getContext();
  },
  present: (action) => {
    if (entryController.getSnapshot().kind === "catalog") throw new ViewerPresentationError("unavailable", "The project catalog has no workflow canvas.");
    return presentation.present(action);
  },
  subscribe: (changed) => {
    const entry = entryController.subscribe(changed);
    const workflow = controller.subscribe(changed);
    return () => { entry(); workflow(); };
  },
}, externalFiles);
// Same-realm browser automation and trusted host preload code use this page facade.
Object.defineProperty(window, "fx_viewer", { configurable: true, value: Object.freeze({
  getContext: page.getContext, present: page.present, subscribe: page.subscribe, allowHost: page.allowHost,
  handleExternalFile: externalFiles.handleExternalFile,
}) });
if (import.meta.hot) import.meta.hot.dispose(() => {
  stopFiles(); stopInventory(); fileInventory.dispose(); page.dispose(); externalFiles.dispose();
});
const iconProps = { size: 14, strokeWidth: 1.5, "aria-hidden": true } as const;

/** A titled set of named values, after `children`; nothing when the set is empty. */
function Section({ title, values, artifacts, children, videoPreview }: { title: string; values: Record<string, unknown>; artifacts: Map<string, Artifact>; children?: ReactNode; videoPreview?: CanvasPreview }) {
  return Object.keys(values).length > 0 && <section className="fx-detail-section"><h3>{title}</h3>{children}<Values values={values} artifacts={artifacts} videoPreview={videoPreview} externalFiles={externalFiles} /></section>;
}

/** Source access is a host capability; standalone labels need no source-serving endpoint. */
function SourceFile({ instanceId, scopeId }: { instanceId?: string; scopeId?: string }) {
  const { available } = useSyncExternalStore(externalFiles.subscribe, externalFiles.getSnapshot);
  const { files } = useSyncExternalStore(fileInventory.subscribe, fileInventory.getSnapshot);
  if (!available) return null;
  const file = files.find((item) => item.target.kind === "source" && (instanceId
    ? item.role === "node" && item.instance_id === instanceId
    : scopeId ? item.role === "workflow" && item.scope_id === scopeId
      : item.role === "workflow" && item.scope_id === undefined && item.instance_id === undefined));
  if (!file || file.target.kind !== "source") return null;
  const { source, symbol } = file.target;
  return <p className="fx-detail-meta"><ExternalFileLink controller={externalFiles} target={file.target} className="fx-source-link" title="Open source outside the viewer">{source}{symbol && `#${symbol}`}</ExternalFileLink></p>;
}

/** Copies `text`, and says so for a moment. */
function CopyButton({ text, label, visible = false }: { text: string; label: string; visible?: boolean }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const reset = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(reset);
  }, [copied]);
  const copy = () => void navigator.clipboard?.writeText(text).then(() => setCopied(true), () => setCopied(false));
  return <button type="button" onClick={copy} className={visible ? "fx-action" : "fx-copy"} aria-label={copied ? "Copied" : label} title={copied ? "Copied" : label}>
    {copied ? <Check {...iconProps} /> : <Copy {...iconProps} />}
    {visible && (copied ? "Copied" : label)}
  </button>;
}

/**
 * A detail's title, state and facts, and the identifier FX itself gives it: a step's instance id,
 * a pending repeat's or a scope's path, a run's name, a workflow's id. The same text names it in
 * the run's events and `inspect --json`, so it can be copied into a question about it.
 */
function Heading({ title, state, meta, id, kind = "id" }: { title: string; state?: string; meta?: (string | false | null | undefined)[]; id?: string; kind?: string }) {
  const line = meta?.filter(Boolean).join(" · ");
  return <div className="fx-detail-heading">
    <div className="fx-detail-heading-row"><h2>{title}</h2>{state && <Status state={state} />}</div>
    {id && <p className="fx-detail-id"><code title={id}>{id}</code><CopyButton text={id} label={`Copy ${kind}`} /></p>}
    {line && <p className="fx-detail-meta">{line}</p>}
  </div>;
}

function Dot() { return <span className="fx-bar-dot" aria-hidden="true">·</span>; }

/** How long the run took, after a dot; an unfinished invocation counts up from its last record. */
function RunTime({ run }: { run: ViewerRun }) {
  const recorded = run.recorded_at ? Date.parse(run.recorded_at) : NaN;
  const live = ["unfinished", "pausing"].includes(run.state) && !Number.isNaN(recorded);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!live) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [live]);
  if (run.elapsed_ms === undefined) return null;
  const elapsed = run.elapsed_ms + (live ? Math.max(0, now - recorded) : 0);
  return <><Dot /><span className="fx-bar-cost" title={live ? "Running for" : "Took"}>{elapsedLabel(elapsed)}</span></>;
}

function duration(ms: number) {
  if (ms < 1000) return `${ms} ms`;
  const seconds = Math.round(ms / 100) / 10;
  return seconds < 60 ? `${seconds} s` : `${Math.floor(seconds / 60)}m ${String(Math.round(seconds % 60)).padStart(2, "0")}s`;
}

/** Literal settings and wired inputs, split by the node's declared settings. */
function split(values: Record<string, unknown>, node?: CanvasStep) {
  const settings = new Set(node?.ports?.settings.map((setting) => setting.name));
  const entries = Object.entries(values);
  return [Object.fromEntries(entries.filter(([name]) => !settings.has(name))), Object.fromEntries(entries.filter(([name]) => settings.has(name)))];
}

function Problem({ children }: { children: ReactNode }) {
  return <p role="alert" className="fx-detail-problem">{children}</p>;
}

/** Download all (.zip): the run's output files the viewer can serve, named as in its `outputs/`. */
function OutputsDownload({ run, artifacts }: { run: ViewerRun; artifacts: Map<string, Artifact> }) {
  const files = outputFiles(run.outputs, (digest) => { const artifact = artifacts.get(digest); return artifact?.available ? artifact.url : null; });
  const archive = useMemo(() => new OutputArchiveController(), []);
  const { building, error } = useSyncExternalStore(archive.subscribe, archive.getSnapshot);
  useEffect(() => () => archive.dispose(), [archive]);
  const left = files.filter((file) => file.url === null).length;
  if (left === files.length) return null;
  return <div className="fx-detail-download">
    <button onClick={() => void archive.download(files, run.run_name)} disabled={building} className="fx-action"><Download {...iconProps} />{building ? "Zipping…" : "Download all (.zip)"}</button>
    {left > 0 && <p className="fx-value-note">{left} {left === 1 ? "file is" : "files are"} not available and left out.</p>}
    {error && <Problem>{error}</Problem>}
  </div>;
}

function PlanDetails({ plan, instance, pending, canvasNode }: { plan: GraphDocument; instance?: GraphInstance; pending?: PendingRepeat; canvasNode?: CanvasStep }) {
  const none = new Map<string, Artifact>();
  if (pending) return <Heading title={pending.path} id={pending.path} kind="step path" state="unexpanded" meta={[`up to ${pending.max} items`, pending.high_usd === null ? "cost unknown" : pending.high_usd > 0 && `at most ${money(pending.high_usd)}`]} />;
  if (!instance) return <>
    <Heading title={plan.workflow.title} id={plan.workflow.id} kind="workflow id" meta={[plan.estimate.ceiling_usd !== null && `ceiling ${money(plan.estimate.ceiling_usd)}`]} />
    <SourceFile />
    {plan.workflow.description && <p className="fx-detail-copy">{plan.workflow.description}</p>}
    <Section title="Inputs" values={plan.inputs ?? {}} artifacts={none} />
  </>;
  const meta = plan.steps?.[instance.step];
  const [inputs, settings] = split(instance.with, canvasNode);
  return <>
    <Heading title={meta?.title || instance.path} id={instance.id} kind="step id" state={instance.state} meta={[priceLabel(instance.price, money)]} />
    <SourceFile instanceId={instance.id} />
    {meta?.description && <p className="fx-detail-copy">{meta.description}</p>}
    {instance.reason && <Problem>{instance.reason}</Problem>}
    <Section title="Routes" values={Object.fromEntries(Object.entries(instance.routes).map(([capability, route]) => [capability, route.route]))} artifacts={none} />
    <Section title="Inputs" values={inputs} artifacts={none} />
    <Section title="Settings" values={settings} artifacts={none} />
  </>;
}

function RunDetails({ run, node, canvasNode, onInspectSource }: { run: ViewerRun; node?: ViewerRun["nodes"][number]; canvasNode?: CanvasStep; onInspectSource?: (id: string) => void }) {
  const artifacts = new Map(run.artifacts.map((artifact) => [artifact.digest, artifact]));
  if (!node) return <>
    <Heading title={run.workflow.title} id={run.run_name} kind="run name" meta={[run.workflow.id]} />
    <SourceFile />
    {run.workflow.description && <p className="fx-detail-copy">{run.workflow.description}</p>}
    <Section title="Outputs" values={run.outputs} artifacts={artifacts}><OutputsDownload run={run} artifacts={artifacts} /></Section>
    <Section title="Inputs" values={run.inputs} artifacts={artifacts} />
  </>;
  const [inputs, settings] = split(node.with, canvasNode);
  const skipped = node.state === "skipped" ? skipExplanation(node.skip_reason, run.nodes) : null;
  return <>
    <Heading title={node.title || node.path} id={node.id} kind="step id" state={node.state} meta={[canvasNode?.review_label, node.cache === "hit" && "cached", node.state === "waiting" && node.waiting_for && `for ${node.waiting_for}`,
      node.duration_ms !== null && duration(node.duration_ms), !!node.waited_ms && `waited ${duration(node.waited_ms)} for a route`]} />
    <SourceFile instanceId={node.id} />
    {skipped ? <section className="fx-detail-skip" aria-label="Why skipped">
      <h3>Why skipped</h3><p>{skipped.message}</p>
      {skipped.related.map((source) => <button key={source.id} type="button" className="fx-detail-skip-source" title={source.id} onClick={() => onInspectSource?.(source.id)}>View {source.label} <ArrowUpRight {...iconProps} /></button>)}
      {node.error && <details className="fx-detail-skip-diagnostic"><summary>Recorded diagnostic</summary><p>{node.error}</p></details>}
    </section> : node.error && <Problem>{node.error}</Problem>}
    {node.routes && <Section title="Routes" values={Object.fromEntries(Object.entries(node.routes).map(([name, route]) => [name, route.route]))} artifacts={artifacts} />}
    <Section title="Outputs" values={node.outputs} artifacts={artifacts} videoPreview={canvasNode?.preview} />
    <Section title="Inputs" values={inputs} artifacts={artifacts} />
    <Section title="Settings" values={settings} artifacts={artifacts} />
  </>;
}

function NoteDetails({ note }: { note: WorkflowNote }) {
  const { available } = useSyncExternalStore(externalFiles.subscribe, externalFiles.getSnapshot);
  const { files } = useSyncExternalStore(fileInventory.subscribe, fileInventory.getSnapshot);
  const source = available && note.source ? files.find((file) => file.target.kind === "source" && file.target.source === note.source!.path && file.target.recorded_digest === note.source!.digest) : null;
  return <section className="fx-note-details">
    <Heading title={note.name} meta={["Note"]} />
    {note.source && <p className="fx-detail-meta">{source ? <ExternalFileLink controller={externalFiles} target={source.target} className="fx-source-link" title="Open note source outside the viewer">{note.source.path}</ExternalFileLink> : note.source.path}</p>}
    <NoteContent note={note} />
  </section>;
}

function ValueDetails({ value, view }: { value: ValueView; view: GraphDocument | ViewerRun }) {
  const inventory = new Map(view.kind === "fx-viewer-run-v1" ? view.artifacts.map((artifact) => [artifact.digest, artifact]) : []);
  const artifacts = valueArtifacts(value.data, inventory);
  return <section className="fx-value-details">
    <Heading title={value.name} meta={[valueKindLabel(value.kind), value.context]} />
    {value.state === "known" ? <Value value={valueInspectionData(value.data)} artifacts={artifacts} externalFiles={externalFiles} /> : <p className="fx-value-note">{valueStateLabel(value)}</p>}
    {value.diagnostic && <Problem>{value.diagnostic}</Problem>}
  </section>;
}

function ScopeDetails({ node }: { node: CanvasStep }) {
  return <>
    <Heading title={node.title} id={node.path ?? node.source_id} kind="path" state={node.kind === "boundary" ? undefined : node.state} meta={[node.kind === "workflow" && `${node.child_count ?? 0} steps`, !!node.failure_count && `${node.failure_count} failed`]} />
    {node.kind === "workflow" && node.scope_id && <SourceFile scopeId={node.scope_id} />}
    {node.errors?.map((error) => <Problem key={error.id}><span className="block font-medium">{error.title}</span>{error.message}</Problem>)}
  </>;
}

/** Floating chrome belongs to this page; the canvas receives only its measured insets. */
function canvasInset(pane: HTMLElement) {
  const box = pane.getBoundingClientRect();
  const edge = (selector: string) => pane.querySelector(selector)?.getBoundingClientRect();
  return {
    left: Math.max(0, (edge(".fx-steps")?.right ?? box.left) - box.left),
    right: Math.max(0, box.right - (edge(".fx-details")?.left ?? box.right)),
    top: Math.max(0, (edge(".fx-bar")?.bottom ?? box.top) - box.top),
    bottom: 0,
  };
}

function usePageMetadata(titleParts: (string | undefined)[], description?: string) {
  const title = [...titleParts.filter(Boolean), "FX Viewer"].join(" · ");
  useEffect(() => {
    document.title = title;
    if (!description) return;
    const meta = document.querySelector<HTMLMetaElement>('meta[name="description"]')!;
    const previous = meta.content;
    meta.content = description;
    return () => { meta.content = previous; };
  }, [title, description]);
}

function AttemptHistory({ history, error, onClose }: { history: RunHistory; error: string | null; onClose: () => void }) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus({ preventScroll: true }); }, []);
  return <aside className="fx-details fx-attempt-history" id="fx-attempt-history" aria-labelledby="fx-attempt-history-title" data-fx-owns-keys
    onKeyDown={event => {
      if (event.key === "Escape" && !event.defaultPrevented && !event.currentTarget.ownerDocument.fullscreenElement) {
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }
    }}>
    <div className="fx-detail-heading">
      <div className="fx-detail-heading-row"><h2 ref={heading} tabIndex={-1} id="fx-attempt-history-title">Attempt history</h2></div>
    </div>
    <button type="button" className="fx-details-close" aria-label="Close run history" onClick={onClose}><X {...iconProps} /></button>
    {error && <p role="alert" className="fx-detail-problem">{error} Showing the last loaded history.</p>}
    <ul className="fx-attempt-history-list">{history.attempts.map((attempt, index) => <li key={attempt.attempt_number}>
      <a className="fx-attempt-row" href={attempt.url} aria-current={attempt.attempt_number === history.selected_attempt ? "page" : undefined}>
        <span>Attempt {attempt.attempt_number}</span>
        {index === 0 && history.latest_available && <span className="fx-attempt-latest">Latest</span>}
        <Status state={attempt.state} />
      </a>
    </li>)}</ul>
    <div className="fx-attempt-share">
      {history.latest_available ? <>
        <CopyButton text={new URL(history.latest_url, window.location.origin).href} label="Copy run link" visible />
        <span>Opens the latest attempt</span>
      </> : <span>Latest attempt unavailable</span>}
    </div>
  </aside>;
}

function WorkflowView() {
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  const external = useSyncExternalStore(externalFiles.subscribe, externalFiles.getSnapshot);
  const inventory = useSyncExternalStore(fileInventory.subscribe, fileInventory.getSnapshot);
  const reviews = useMemo(() => new WaitingReviewController(apiBase), []);
  const pendingAnswers = useSyncExternalStore(reviews.subscribe, reviews.getPendingCount);
  const reviewStep = useSyncExternalStore(reviews.subscribe, reviews.getOpenStep);
  const historyController = useMemo(() => new RunHistoryController(entryController.route), []);
  const { history, error: historyError } = useSyncExternalStore(historyController.subscribe, historyController.getSnapshot);
  const [historyOpen, setHistoryOpen] = useState(false);
  const historyToggle = useRef<HTMLButtonElement>(null);
  const actionController = useMemo(() => new RunActionsController(entryController.route, {
    onAccepted: (action, receipt) => {
      if (action === "rerun" && receipt.url) window.location.assign(receipt.url);
      else void controller.refresh();
    },
  }), []);
  const actionState = useSyncExternalStore(actionController.subscribe, actionController.getSnapshot);
  const [actionsOpen, setActionsOpen] = useState(false);
  const actionsToggle = useRef<HTMLButtonElement>(null);
  useEffect(() => { historyController.setOpen(historyOpen); }, [historyOpen, historyController]);
  useEffect(() => { actionController.setOpen(actionsOpen); }, [actionsOpen, actionController]);
  const closeHistory = useCallback(() => {
    setHistoryOpen(false);
    historyToggle.current?.focus({ preventScroll: true });
  }, []);
  const closeActions = useCallback(() => {
    setActionsOpen(false);
    actionsToggle.current?.focus({ preventScroll: true });
  }, []);
  useEffect(() => {
    const favicon = new RunFavicon(document.querySelector<HTMLLinkElement>('link[rel="icon"]')!);
    let pageActive = true;
    const syncIcon = () => {
      if (!pageActive) favicon.setState("idle");
      else if (reviews.getPendingCount() > 0) favicon.setState("awaiting");
      else favicon.setState(controller.getSnapshot().runActive ? "active" : "idle");
    };
    const unsubscribe = controller.subscribe(syncIcon);
    const unsubscribeReviews = reviews.subscribe(syncIcon);
    const hide = () => {
      pageActive = false;
      controller.dispose();
      historyController.dispose();
      actionController.dispose();
      syncIcon();
    };
    const show = (event: PageTransitionEvent) => {
      if (!event.persisted) return;
      pageActive = true;
      void controller.refresh();
      historyController.start();
      actionController.start();
      syncIcon();
    };
    window.addEventListener("pagehide", hide);
    window.addEventListener("pageshow", show);
    void controller.refresh();
    historyController.start();
    actionController.start();
    return () => {
      window.removeEventListener("pagehide", hide);
      window.removeEventListener("pageshow", show);
      unsubscribe();
      unsubscribeReviews();
      controller.dispose();
      historyController.dispose();
      actionController.dispose();
      favicon.dispose();
    };
  }, [reviews, historyController, actionController]);
  const { view, graph, layout, selected, loading, error, scope, breadcrumbs, scopeNode, cancelling, runActive, invocationId } = state;
  // The workflow's own details, opened from its name; a selection or another workflow closes them.
  const [about, setAbout] = useState(false);
  useEffect(() => { setAbout(false); setHistoryOpen(false); setActionsOpen(false); reviews.close(); }, [scope, reviews]);
  const select = (id: string | null) => { reviews.close(); setAbout(false); setHistoryOpen(false); setActionsOpen(false); controller.select(id); };
  const plan = view && view.kind !== "fx-viewer-run-v1" ? view : null;
  const run = view?.kind === "fx-viewer-run-v1"
    ? view.state === "pausing" && !runActive ? { ...view, state: "unfinished" } : view
    : null;
  const hasReviews = run?.nodes.some(node => /^fx\/await@0*1$/.test(node.uses ?? "")) ?? false;
  const reviewOpen = hasReviews && reviewStep !== null;
  const reviewCards = useSyncExternalStore(reviews.subscribe, reviews.getCards);
  const liveGraph = useMemo(() => hasReviews ? withReviewCards(graph, reviewCards) : graph, [graph, hasReviews, reviewCards]);
  let runStatus = run?.state ?? "";
  if (cancelling) runStatus = "cancelling";
  else if (reviewCards.size > 0 && (runStatus === "unfinished" || runStatus === "awaiting")) {
    runStatus = pendingAnswers > 0 ? "awaiting" : "unfinished";
  }
  useEffect(() => {
    if (!hasReviews) return;
    reviews.start();
    return () => reviews.stop();
  }, [hasReviews, reviews]);
  const workflowTitle = view?.workflow.title.trim() || view?.workflow.id;
  const description = view?.workflow.description?.trim()
    || (plan ? `Inspect the execution plan for ${workflowTitle}.`
      : run ? `Inspect run ${run.run_name} for ${workflowTitle}.` : undefined);
  const runTitle = pendingAnswers === 1 ? "Awaiting answer" : pendingAnswers > 1 ? `Awaiting ${pendingAnswers} answers`
    : runStatus === "paused" ? "Paused" : runStatus === "pausing" ? "Pausing" : "Run";
  usePageMetadata([run ? runTitle : plan ? "Plan" : undefined, workflowTitle, run?.run_name], description);
  const selectedNote = graph.notes?.find((note) => note.id === selected);
  const selectedNode = graph.nodes.find((node) => node.id === selected);
  const stepNodes = graph.nodes.filter((node) => node.kind !== "value");
  const instance = plan?.instances.find((node) => !selectedNode?.pending && node.id === selectedNode?.source_id);
  const pending = plan?.pending.find((node) => selectedNode?.pending && node.path === selectedNode.source_id);
  const runNode = run?.nodes.find((node) => node.id === selectedNode?.source_id);
  const notices = [...(plan?.problems?.map((problem) => `${problem.where}: ${problem.message}`) ?? []), ...(run?.warnings ?? [])];
  const open = selectedNode !== undefined || selectedNote !== undefined || about;
  // The step list is a popover: hidden until asked for, closed by a press anywhere else.
  const [steps, setSteps] = useState(false);
  const toggle = useRef<HTMLButtonElement>(null);
  const popover = useRef<HTMLElement>(null);
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!steps) return;
    const outside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!popover.current?.contains(target) && !toggle.current?.contains(target)) setSteps(false);
    };
    document.addEventListener("pointerdown", outside, { capture: true });
    return () => document.removeEventListener("pointerdown", outside, { capture: true });
  }, [steps]);
  useEffect(() => { list.current?.querySelector('[aria-current="page"]')?.scrollIntoView({ block: "nearest" }); }, [selected, steps]);
  // A step chosen from the list brings its card into the canvas the open panels leave free.
  const pane = useRef<HTMLElement>(null);
  const canvas = useRef<WorkflowCanvasHandle>(null);
  const closeReview = useCallback(() => {
    const step = reviews.getOpenStep();
    reviews.close();
    if (!step || !pane.current) return;
    const card = pane.current.querySelector<SVGElement>(`[data-node-id="${CSS.escape(`instance:${step}`)}"]`);
    const action = card?.querySelector<SVGElement>(".fx-canvas-review-action");
    if (action?.getClientRects().length) action.focus({ preventScroll: true });
    else pane.current.querySelector<HTMLElement>(".fx-canvas")?.focus({ preventScroll: true });
  }, [reviews]);
  const canvasLoaded = view !== null;
  useEffect(() => {
    if (!canvasLoaded) return;
    return presentation.attachCanvas((next, focus) => {
      // Commit the controller's selection/scope and its details before measuring the free area.
      flushSync(() => {
        setAbout(false);
        if (focus !== null) {
          setHistoryOpen(false);
          setActionsOpen(false);
          setSteps(false);
          reviews.close();
        }
      });
      if (!canvas.current || !pane.current) throw new ViewerPresentationError("not_ready", "The workflow canvas is not mounted.");
      const graph = hasReviews ? withReviewCards(next.graph, reviews.getCards()) : next.graph;
      canvas.current.present(graph, next.layout, next.selected, next.scope, focus, canvasInset(pane.current));
    });
  }, [canvasLoaded, hasReviews, reviews]);
  const [reveal, setReveal] = useState<{ id: string } | null>(null);
  const inspectSource = (id: string) => {
    if (!run?.nodes.some((node) => node.id === id)) return;
    controller.goToScope(null);
    for (const scope of skipSourceScopes(id, run.scopes)) controller.openScope(scope);
    const selected = `instance:${id}`;
    select(selected);
    setReveal({ id: selected });
  };
  useEffect(() => {
    if (!reveal || !pane.current) return;
    canvas.current?.reveal(reveal.id, canvasInset(pane.current));
  }, [reveal]);
  // Esc closes the right-hand panel before the step list. Interactive content can handle it first.
  useEffect(() => {
    if (!reviewOpen && !historyOpen && !actionsOpen && !open && !steps) return;
    const close = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented || ownsKeys(event.target)) return;
      event.preventDefault();
      if (reviewOpen) closeReview();
      else if (historyOpen) closeHistory();
      else if (actionsOpen) closeActions();
      else if (open) { setAbout(false); controller.select(null); }
      else setSteps(false);
    };
    document.addEventListener("keydown", close, { capture: true });
    return () => document.removeEventListener("keydown", close, { capture: true });
  }, [reviewOpen, historyOpen, actionsOpen, open, steps, closeReview, closeHistory, closeActions]);
  return <div className="fx-viewer">
    {error && <p role="alert" className="fx-error">{error}{view && " Showing the last loaded data."}</p>}
    {external.error && <p role="alert" className="fx-error">{external.error}{external.failedReference?.target.kind === "artifact" && external.failedReference.target.artifact.url && <>{" "}<a href={external.failedReference.target.artifact.url} target="_blank" rel="noopener noreferrer" className="underline">Open in browser</a></>}{" "}<button type="button" className="underline" onClick={externalFiles.clearError}>Dismiss</button></p>}
    {external.available && inventory.error && <p role="alert" className="fx-error">{inventory.error}{" "}<button type="button" className="underline" onClick={() => void fileInventory.refresh()}>Retry source files</button></p>}
    {!view ? <main className="fx-unavailable"><h2>{loading ? "Opening the workflow…" : "Workflow unavailable"}</h2><p>{loading ? "Reading the selected graph or run." : "Check that the local viewer server is running."}</p>{!loading && <button onClick={() => void controller.refresh()} className="fx-action">Try again</button>}</main>
      : <main ref={pane} aria-label="Workflow canvas" className="fx-canvas-pane">
        <WorkflowCanvas ref={canvas} graph={liveGraph} layout={layout} selected={selected} scope={scope} onSelect={select}
          onOpenScope={controller.openScope} onBack={controller.back} onReview={hasReviews ? step => { setHistoryOpen(false); setActionsOpen(false); reviews.open(step); } : undefined} />
        <nav aria-label="Workflow location" className="fx-bar">
          {entryController.route?.project_id && <a href="/" className="fx-bar-project" title="Project" aria-label="Project"><ArrowLeft {...iconProps} /></a>}
          <ol className="fx-crumbs">{breadcrumbs.map((crumb, index) => <li key={crumb.id ?? "root"}>{index < breadcrumbs.length - 1
            ? <button onClick={() => controller.goToScope(crumb.id)} title={crumb.id ?? crumb.title}>{crumb.title}</button>
            : <button onClick={() => { select(null); setAbout(reviewOpen || !about); }} aria-current="page" aria-expanded={!reviewOpen && about}>{crumb.title}</button>}</li>)}</ol>
          {run ? <><StateLabel state={runStatus} /><RunTime run={run} />{run.charged_usd !== null && <><Dot /><span className="fx-bar-cost" title="Budget booked; includes reserved amounts when the provider reports no final charge">booked {money(run.charged_usd)}</span></>}</>
            : view.estimate && <span className="fx-bar-cost">{priceLabel(view.estimate, money, "est. $0.00")}</span>}
          {(run?.stand_in || plan?.stand_in) && <span className="fx-badge">Stand-in</span>}
          {notices.length > 0 && <button onClick={() => { select(null); setAbout(true); }} className="fx-bar-notice" title={notices.join("\n")}>{notices.length} {plan ? (notices.length === 1 ? "problem" : "problems") : (notices.length === 1 ? "notice" : "notices")}</button>}
          {run && history && <button type="button" ref={historyToggle} className="fx-bar-history" title="Run history"
            aria-label={`Attempt ${history.selected_attempt}, run history`} aria-expanded={historyOpen} aria-controls={historyOpen ? "fx-attempt-history" : undefined}
            onClick={() => {
              if (historyOpen) closeHistory();
              else { select(null); setHistoryOpen(true); }
            }}><History {...iconProps} />Attempt {history.selected_attempt}</button>}
          {run && actionState.control && <button type="button" ref={actionsToggle} className="fx-bar-controls" title="Run actions" aria-label="Run actions"
            aria-expanded={actionsOpen} aria-controls={actionsOpen ? "fx-run-actions" : undefined}
            onClick={() => {
              if (actionsOpen) closeActions();
              else { select(null); actionController.setOpen(true); setActionsOpen(true); }
            }}><Ellipsis {...iconProps} /></button>}
        </nav>
        <button ref={toggle} onClick={() => setSteps(!steps)} className="fx-steps-toggle" aria-expanded={steps} aria-controls="fx-steps" aria-label="Steps" title="Steps"><List {...iconProps} size={16} /></button>
        {steps && <nav ref={popover} id="fx-steps" aria-label="Workflow steps" className="fx-steps">
          <p className="fx-steps-heading"><span>Steps</span><span>{stepNodes.length}</span></p>
          <div ref={list} className="fx-steps-list">{stepNodes.map((node) => <button key={node.id} onClick={() => { select(node.id); setReveal({ id: node.id }); }} aria-current={selected === node.id ? "page" : undefined} data-state={node.state} aria-label={`${node.title}, ${node.state}`} title={node.source_id} className="fx-step-row">
            <span className="fx-step-label">{node.title}</span><StateIcon state={node.state} />
          </button>)}</div>
        </nav>}
        {run && reviewOpen ? <WaitingReviews run={run} controller={reviews} onClose={closeReview} />
        : history && historyOpen ? <AttemptHistory history={history} error={historyError} onClose={closeHistory} />
        : actionsOpen ? <RunActionsPanel state={actionState} controller={actionController} invocationId={invocationId} onClose={closeActions} />
        : open && <aside aria-label="Details" className="fx-details">
          <button onClick={() => select(null)} className="fx-details-close" aria-label="Close details" title="Close"><X {...iconProps} /></button>
          {selectedNote ? <NoteDetails note={selectedNote} /> : selectedNode?.value ? <ValueDetails value={selectedNode.value} view={view} /> : selectedNode?.kind ? <ScopeDetails node={selectedNode} />
            : selectedNode ? plan ? <PlanDetails plan={plan} instance={instance} pending={pending} canvasNode={selectedNode} />
              : selectedNode.pending ? <Heading title={selectedNode.title} id={selectedNode.source_id} kind="step path" state={selectedNode.state} meta={[selectedNode.subtitle]} />
              : <RunDetails run={run!} node={runNode} canvasNode={selectedNode} onInspectSource={inspectSource} />
            : scopeNode ? <ScopeDetails node={scopeNode} />
            : plan ? <PlanDetails plan={plan} /> : <RunDetails run={run!} />}
          {!selectedNode && !selectedNote && <>
            {plan?.problems?.map((problem, index) => <Problem key={index}><span className="block font-mono">{problem.where}</span>{problem.message}</Problem>)}
            {run?.warnings.map((warning, index) => <p key={index} className="fx-detail-notice">{warning}</p>)}
          </>}
        </aside>}
      </main>}
  </div>;
}

function EntryTime({ entry }: { entry: ServiceEntry }) {
  return entry.created_at
    ? <time dateTime={entry.created_at} title={entry.created_at}>{new Date(entry.created_at).toLocaleString(undefined, { month: "short", day: "numeric", year: "numeric", hour: "numeric", minute: "2-digit", second: "2-digit" })}</time>
    : <span>Creation time not recorded</span>;
}

function HistoryEntry({ entry, latest = false }: { entry: ServiceEntry; latest?: boolean }) {
  return <li><a href={entry.url} className="fx-project-history-entry">
    {entry.kind === "plan" ? <FileText {...iconProps} /> : <Square {...iconProps} />}
    <code>{entry.name ?? entry.id.slice(0, 12)}</code>{entry.attempt_number !== undefined && <span>Attempt {entry.attempt_number}</span>}{latest && <span className="fx-project-latest">Latest</span>}
    <span className="fx-project-history-time"><EntryTime entry={entry} /></span><Status state={entry.state} /><ArrowUpRight {...iconProps} />
  </a></li>;
}

function ProjectIndex({ catalog, loading }: { catalog: ServiceIndex; loading: boolean }) {
  usePageMetadata(["Project", catalog.title]);
  const { groups } = new ServiceIndexProjection(catalog);
  return <main className="fx-project">
    <header className="fx-project-heading"><h1>{catalog.title}</h1><button onClick={() => void entryController.refresh()} disabled={loading} className="fx-action" aria-label="Refresh project"><RefreshCw {...iconProps} />Refresh</button></header>
    <div className="fx-project-list-heading"><h2>Workflows</h2><span>{groups.length} {groups.length === 1 ? "workflow" : "workflows"}</span></div>
    {groups.length ? <ul className="fx-project-list">{groups.map((group) => <li key={group.key}>
      <a href={group.primary.url} className="fx-project-entry">
        <Workflow {...iconProps} /><div className="fx-project-entry-name"><h3>{group.title}</h3>
          {group.show_source && <code className="fx-project-source" title={group.workflow?.source}>{group.workflow?.source}</code>}
          <span>{group.primary.kind === "plan" ? "Saved plan" : group.latest_known ? "Latest run" : "Run"}<code>{group.primary.name ?? group.primary.id.slice(0, 12)}</code>{(group.primary.attempt_number ?? 0) > 1 && <span>Attempt {group.primary.attempt_number}</span>}<EntryTime entry={group.primary} /></span>
        </div><Status state={group.primary.state} /><ArrowUpRight {...iconProps} />
      </a>
      {group.unfinished_runs.length > 0 && <section className="fx-project-unfinished" aria-label={`Other unfinished runs of ${group.title}`}><p>Other unfinished runs</p><ul>{group.unfinished_runs.map((entry) => <HistoryEntry key={entry.id} entry={entry} />)}</ul></section>}
      {(group.runs.length > 0 || group.plans.length > 1) && <details className="fx-project-history"><summary><ChevronRight {...iconProps} /><span>History</span><span className="fx-project-history-count">{group.runs.length > 0 && `${group.runs.length} ${group.runs.length === 1 ? "run" : "runs"}`}{group.runs.length > 0 && group.plans.length > 0 && " · "}{group.plans.length > 0 && `${group.plans.length} ${group.plans.length === 1 ? "saved plan" : "saved plans"}`}</span></summary>
        {group.runs.length > 0 && <div><h4>Runs</h4><ul>{group.runs.map((entry) => <HistoryEntry key={entry.id} entry={entry} latest={group.latest_known && entry === group.primary} />)}</ul></div>}
        {group.plans.length > 0 && <div><h4>Saved plans</h4><ul>{group.plans.map((entry) => <HistoryEntry key={entry.id} entry={entry} />)}</ul></div>}
      </details>}
    </li>)}</ul> : <div className="fx-project-empty"><Workflow size={24} strokeWidth={1.25} aria-hidden="true" /><h2>No workflows yet</h2><p>Run a workflow or use plan --open in this project to see it here.</p></div>}
  </main>;
}

function App() {
  const { kind, catalog, loading, error } = useSyncExternalStore(entryController.subscribe, entryController.getSnapshot);
  useEffect(() => { void entryController.refresh(); return () => entryController.dispose(); }, []);
  if (kind === "viewer") return <WorkflowView />;
  return <div className="fx-viewer">
    {error && <p role="alert" className="fx-error">{error}{catalog && " Showing the last loaded data."}</p>}
    {catalog ? <ProjectIndex catalog={catalog} loading={loading} /> : <main className="fx-unavailable"><h2>{loading ? "Opening FX…" : "FX unavailable"}</h2><p>{loading ? "Reading the local project or workflow." : "Check that the local FX service is running."}</p>{!loading && <button onClick={() => void entryController.refresh()} className="fx-action">Try again</button>}</main>}
  </div>;
}

createRoot(document.getElementById("root")!).render(<StrictMode><App /></StrictMode>);
