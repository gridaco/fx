import { StrictMode, useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { ArrowLeft, ArrowUpRight, Check, ChevronRight, Copy, FileText, List, RefreshCw, Square, Workflow, X } from "lucide-react";
import { ServiceIndexProjection, ViewerController, ViewerEntryController, type Artifact, type CanvasStep, type GraphDocument, type GraphInstance, type PendingRepeat, type ServiceEntry, type ServiceIndex, type ViewerRun } from "@grida/fx-web";
import { StateIcon, Status, Values, WorkflowCanvas, money, type WorkflowCanvasHandle } from "@grida/fx-react";
import "./style.css";

const entryController = new ViewerEntryController(window.location.pathname);
const apiBase = entryController.route?.api_base ?? "/api";
const controller = new ViewerController(undefined, { apiBase });
const iconProps = { size: 14, strokeWidth: 1.5, "aria-hidden": true } as const;

/** A titled set of named values; nothing when the set is empty. */
function Section({ title, values, artifacts }: { title: string; values: Record<string, unknown>; artifacts: Map<string, Artifact> }) {
  return Object.keys(values).length > 0 && <section className="fx-detail-section"><h3>{title}</h3><Values values={values} artifacts={artifacts} /></section>;
}

/** Copies `text`, and says so for a moment. */
function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const reset = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(reset);
  }, [copied]);
  const copy = () => void navigator.clipboard?.writeText(text).then(() => setCopied(true), () => setCopied(false));
  return <button type="button" onClick={copy} className="fx-copy" aria-label={copied ? "Copied" : label} title={copied ? "Copied" : label}>
    {copied ? <Check {...iconProps} /> : <Copy {...iconProps} />}
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

function duration(ms: number) {
  if (ms < 1000) return `${ms} ms`;
  const seconds = Math.round(ms / 100) / 10;
  return seconds < 60 ? `${seconds} s` : `${Math.floor(seconds / 60)}m ${String(Math.round(seconds % 60)).padStart(2, "0")}s`;
}

/** "est. $0.40" or "est. $0.40 – $1.20"; nothing for a step that pays for nothing. */
function estimate(price: { low_usd: number; high_usd: number }, free = "") {
  if (price.high_usd === 0) return free;
  return `est. ${money(price.low_usd)}${price.high_usd === price.low_usd ? "" : ` – ${money(price.high_usd)}`}`;
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

function PlanDetails({ plan, instance, pending, canvasNode }: { plan: GraphDocument; instance?: GraphInstance; pending?: PendingRepeat; canvasNode?: CanvasStep }) {
  const none = new Map<string, Artifact>();
  if (pending) return <Heading title={pending.path} id={pending.path} kind="step path" state="unexpanded" meta={[`up to ${pending.max} items`, pending.high_usd > 0 && `at most ${money(pending.high_usd)}`]} />;
  if (!instance) return <>
    <Heading title={plan.workflow.title} id={plan.workflow.id} kind="workflow id" meta={[plan.estimate.ceiling_usd !== null && `ceiling ${money(plan.estimate.ceiling_usd)}`]} />
    {plan.workflow.description && <p className="fx-detail-copy">{plan.workflow.description}</p>}
    <Section title="Inputs" values={plan.inputs ?? {}} artifacts={none} />
  </>;
  const meta = plan.steps?.[instance.step];
  const [inputs, settings] = split(instance.with, canvasNode);
  return <>
    <Heading title={meta?.title || instance.path} id={instance.id} kind="step id" state={instance.state} meta={[estimate(instance.price)]} />
    {meta?.description && <p className="fx-detail-copy">{meta.description}</p>}
    {instance.reason && <Problem>{instance.reason}</Problem>}
    <Section title="Routes" values={Object.fromEntries(Object.entries(instance.routes).map(([capability, route]) => [capability, route.route]))} artifacts={none} />
    <Section title="Inputs" values={inputs} artifacts={none} />
    <Section title="Settings" values={settings} artifacts={none} />
  </>;
}

function RunDetails({ run, node, canvasNode }: { run: ViewerRun; node?: ViewerRun["nodes"][number]; canvasNode?: CanvasStep }) {
  const artifacts = new Map(run.artifacts.map((artifact) => [artifact.digest, artifact]));
  if (!node) return <>
    <Heading title={run.workflow.title} id={run.run_name} kind="run name" meta={[run.workflow.id]} />
    {run.workflow.description && <p className="fx-detail-copy">{run.workflow.description}</p>}
    <Section title="Outputs" values={run.outputs} artifacts={artifacts} />
    <Section title="Inputs" values={run.inputs} artifacts={artifacts} />
  </>;
  const [inputs, settings] = split(node.with, canvasNode);
  return <>
    <Heading title={node.title || node.path} id={node.id} kind="step id" state={node.state} meta={[node.cache === "hit" && "cached", node.state === "waiting" && node.waiting_for && `for ${node.waiting_for}`,
      node.duration_ms !== null && duration(node.duration_ms), !!node.waited_ms && `waited ${duration(node.waited_ms)} for a route`]} />
    {node.error && <Problem>{node.error}</Problem>}
    <Section title="Outputs" values={node.outputs} artifacts={artifacts} />
    <Section title="Inputs" values={inputs} artifacts={artifacts} />
    <Section title="Settings" values={settings} artifacts={artifacts} />
  </>;
}

function ScopeDetails({ node }: { node: CanvasStep }) {
  return <>
    <Heading title={node.title} id={node.path ?? node.source_id} kind="path" state={node.kind === "boundary" ? undefined : node.state} meta={[node.kind === "workflow" && `${node.child_count ?? 0} steps`, !!node.failure_count && `${node.failure_count} failed`]} />
    {node.errors?.map((error) => <Problem key={error.id}><span className="block font-medium">{error.title}</span>{error.message}</Problem>)}
  </>;
}

function WorkflowView() {
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  useEffect(() => { void controller.refresh(); return () => controller.dispose(); }, []);
  const { view, graph, layout, selected, loading, error, scope, breadcrumbs, scopeNode, cancelling } = state;
  // The workflow's own details, opened from its name; a selection or another workflow closes them.
  const [about, setAbout] = useState(false);
  useEffect(() => setAbout(false), [scope]);
  const select = (id: string | null) => { setAbout(false); controller.select(id); };
  const plan = view?.kind === "fx-graph-v1" ? view : null;
  const run = view?.kind === "fx-viewer-run-v1" ? view : null;
  const selectedNode = graph.nodes.find((node) => node.id === selected);
  const instance = plan?.instances.find((node) => !selectedNode?.pending && node.id === selectedNode?.source_id);
  const pending = plan?.pending.find((node) => selectedNode?.pending && node.path === selectedNode.source_id);
  const runNode = run?.nodes.find((node) => node.id === selectedNode?.source_id);
  const notices = [...(plan?.problems?.map((problem) => `${problem.where}: ${problem.message}`) ?? []), ...(run?.warnings ?? [])];
  const open = selectedNode !== undefined || about;
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
  const [reveal, setReveal] = useState<{ id: string } | null>(null);
  useEffect(() => {
    if (!reveal || !pane.current) return;
    const box = pane.current.getBoundingClientRect();
    const edge = (selector: string) => pane.current!.querySelector(selector)?.getBoundingClientRect();
    canvas.current?.reveal(reveal.id, {
      left: Math.max(0, (edge(".fx-steps")?.right ?? box.left) - box.left),
      right: Math.max(0, box.right - (edge(".fx-details")?.left ?? box.right)),
      top: Math.max(0, (edge(".fx-bar")?.bottom ?? box.top) - box.top),
      bottom: 0,
    });
  }, [reveal]);
  // Esc closes the details, then the step list; an open deck takes it before them, and the canvas's own Esc after.
  useEffect(() => {
    if (!open && !steps) return;
    const close = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented || (event.target as Element | null)?.closest?.("input, textarea, select, [contenteditable]")) return;
      event.preventDefault();
      if (open) { setAbout(false); controller.select(null); } else setSteps(false);
    };
    document.addEventListener("keydown", close, { capture: true });
    return () => document.removeEventListener("keydown", close, { capture: true });
  }, [open, steps]);
  return <div className="fx-viewer">
    {error && <p role="alert" className="fx-error">{error}{view && " Showing the last loaded data."}</p>}
    {!view ? <main className="fx-unavailable"><h2>{loading ? "Opening the workflow…" : "Workflow unavailable"}</h2><p>{loading ? "Reading the selected graph or run." : "Check that the local viewer server is running."}</p>{!loading && <button onClick={() => void controller.refresh()} className="fx-action">Try again</button>}</main>
      : <main ref={pane} aria-label="Workflow canvas" className="fx-canvas-pane">
        <WorkflowCanvas ref={canvas} graph={graph} layout={layout} selected={selected} scope={scope} onSelect={select} onOpenScope={controller.openScope} onBack={controller.back} />
        <nav aria-label="Workflow location" className="fx-bar">
          {entryController.route?.project_id && <a href="/" className="fx-bar-project" title="Project" aria-label="Project"><ArrowLeft {...iconProps} /></a>}
          <ol className="fx-crumbs">{breadcrumbs.map((crumb, index) => <li key={crumb.id ?? "root"}>{index < breadcrumbs.length - 1
            ? <button onClick={() => controller.goToScope(crumb.id)} title={crumb.id ?? crumb.title}>{crumb.title}</button>
            : <button onClick={() => { controller.select(null); setAbout(!about); }} aria-current="page" aria-expanded={about}>{crumb.title}</button>}</li>)}</ol>
          {run ? <><Status state={cancelling ? "cancelling" : run.state} />{run.charged_usd !== null && <span className="fx-bar-cost">{money(run.charged_usd)}</span>}</>
            : view.estimate && <span className="fx-bar-cost">{estimate(view.estimate, "est. $0.00")}</span>}
          {(run?.stand_in || plan?.stand_in) && <span className="fx-badge">Stand-in</span>}
          {notices.length > 0 && <button onClick={() => { controller.select(null); setAbout(true); }} className="fx-bar-notice" title={notices.join("\n")}>{notices.length} {plan ? (notices.length === 1 ? "problem" : "problems") : (notices.length === 1 ? "notice" : "notices")}</button>}
        </nav>
        <button ref={toggle} onClick={() => setSteps(!steps)} className="fx-steps-toggle" aria-expanded={steps} aria-controls="fx-steps" aria-label="Steps" title="Steps"><List {...iconProps} size={16} /></button>
        {steps && <nav ref={popover} id="fx-steps" aria-label="Workflow steps" className="fx-steps">
          <p className="fx-steps-heading"><span>Steps</span><span>{graph.nodes.length}</span></p>
          <div ref={list} className="fx-steps-list">{graph.nodes.map((node) => <button key={node.id} onClick={() => { select(node.id); setReveal({ id: node.id }); }} aria-current={selected === node.id ? "page" : undefined} data-state={node.state} aria-label={`${node.title}, ${node.state}`} title={node.source_id} className="fx-step-row">
            <span className="fx-step-label">{node.title}</span><StateIcon state={node.state} />
          </button>)}</div>
        </nav>}
        {open && <aside aria-label="Details" className="fx-details">
          <button onClick={() => select(null)} className="fx-details-close" aria-label="Close details" title="Close"><X {...iconProps} /></button>
          {selectedNode?.kind ? <ScopeDetails node={selectedNode} />
            : selectedNode ? plan ? <PlanDetails plan={plan} instance={instance} pending={pending} canvasNode={selectedNode} />
              : selectedNode.pending ? <Heading title={selectedNode.title} id={selectedNode.source_id} kind="step path" state={selectedNode.state} meta={[selectedNode.subtitle]} />
              : <RunDetails run={run!} node={runNode} canvasNode={selectedNode} />
            : scopeNode ? <ScopeDetails node={scopeNode} />
            : plan ? <PlanDetails plan={plan} /> : <RunDetails run={run!} />}
          {!selectedNode && <>
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
    <code>{entry.name ?? entry.id.slice(0, 12)}</code>{latest && <span className="fx-project-latest">Latest</span>}
    <span className="fx-project-history-time"><EntryTime entry={entry} /></span><Status state={entry.state} /><ArrowUpRight {...iconProps} />
  </a></li>;
}

function ProjectIndex({ catalog, loading }: { catalog: ServiceIndex; loading: boolean }) {
  const { groups } = new ServiceIndexProjection(catalog);
  return <main className="fx-project">
    <header className="fx-project-heading"><h1>{catalog.title}</h1><button onClick={() => void entryController.refresh()} disabled={loading} className="fx-action" aria-label="Refresh project"><RefreshCw {...iconProps} />Refresh</button></header>
    <div className="fx-project-list-heading"><h2>Workflows</h2><span>{groups.length} {groups.length === 1 ? "workflow" : "workflows"}</span></div>
    {groups.length ? <ul className="fx-project-list">{groups.map((group) => <li key={group.key}>
      <a href={group.primary.url} className="fx-project-entry">
        <Workflow {...iconProps} /><div className="fx-project-entry-name"><h3>{group.title}</h3>
          {group.show_source && <code className="fx-project-source" title={group.workflow?.source}>{group.workflow?.source}</code>}
          <span>{group.primary.kind === "plan" ? "Saved plan" : group.latest_known ? "Latest run" : "Run"}<code>{group.primary.name ?? group.primary.id.slice(0, 12)}</code><EntryTime entry={group.primary} /></span>
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
