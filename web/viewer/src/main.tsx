import { StrictMode, useEffect, useSyncExternalStore, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { ArrowLeft, LayoutGrid, RefreshCw, Square, Workflow } from "lucide-react";
import { ViewerController, displayPortName, type CanvasStep, type GraphDocument, type GraphInstance, type InterfaceBinding, type PendingRepeat, type PortBinding, type ViewerRun } from "@grida/fx-web";
import { PortList, Status, ValueList, WorkflowCanvas, money } from "@grida/fx-react";
import "./style.css";

const controller = new ViewerController();
const iconProps = { size: 14, strokeWidth: 1.5, "aria-hidden": true } as const;

function JsonValue({ value }: { value: unknown }) {
  return <pre className="fx-json">{JSON.stringify(value, null, 2)}</pre>;
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return <section className="fx-detail-section"><h3>{title}</h3>{children}</section>;
}

function InspectorHeading({ label, title, code, state }: { label: string; title: string; code?: string; state?: string }) {
  return <div className="fx-detail-heading">
    <div className="fx-detail-heading-row"><p className="fx-detail-kicker">{label}</p>{state && <Status state={state} />}</div>
    <h2>{title}</h2>{code && <p className="fx-detail-code">{code}</p>}
  </div>;
}

function References({ title, ids }: { title: string; ids: string[] }) {
  return <Section title={title}>{ids.length ? <div className="flex flex-wrap gap-2">{ids.map((id) => controller.referenceTarget(id) ? <button key={id} onClick={() => controller.selectReference(id)} className="max-w-full rounded border border-zinc-200 px-2 py-1 text-left font-mono text-[11px] break-all text-zinc-600 hover:border-zinc-400">{id}</button> : <span key={id} className="max-w-full rounded border border-zinc-100 px-2 py-1 font-mono text-[11px] break-all text-zinc-400">{id}</span>)}</div> : <p className="text-xs text-zinc-400">None recorded.</p>}</Section>;
}

function PortSections({ node, bindings, values }: { node?: CanvasStep; bindings?: (PortBinding | InterfaceBinding)[]; values: Record<string, unknown> }) {
  if (!node?.ports) return <Section title="Named ports"><p className="text-xs leading-5 text-zinc-400">Port metadata was not recorded. Connections show only recorded step dependencies.</p></Section>;
  return <>
    <Section title="Input ports"><PortList ports={node.ports.inputs} /></Section>
    <Section title="Output ports"><PortList ports={node.ports.outputs} /></Section>
    <Section title="Settings">{node.ports.settings.length ? <div className="space-y-3">{node.ports.settings.map((setting) => <div key={setting.name}><p className="mb-1.5 font-mono text-[11px] text-zinc-500">{setting.name} <span className="text-zinc-400">· {setting.type}</span></p>{setting.name in values ? <JsonValue value={values[setting.name]} /> : <p className="text-xs text-zinc-400">No value recorded.</p>}</div>)}</div> : <p className="text-xs text-zinc-400">No literal settings.</p>}</Section>
    <Section title="Resolved port connections">{bindings === undefined ? <p className="text-xs text-zinc-400">Port bindings were not recorded.</p> : bindings.length ? <ul className="divide-y divide-zinc-100">{bindings.map((binding, index) => {
      const target = controller.referenceTarget(binding.source, binding.source_kind);
      const source = controller.getSnapshot().graph.nodes.find((candidate) => candidate.id === target);
      const label = `${source?.title ?? binding.source} · ${displayPortName(binding.source_port)}`;
      return <li key={index} className="py-1.5 text-[11px] leading-4">{target ? <button onClick={() => controller.selectReference(binding.source, binding.source_kind)} className="text-left font-mono break-all text-blue-700 hover:underline">{label}</button> : <span className="font-mono break-all text-zinc-500">{label}</span>}{binding.source_kind !== "output" && <span className="ml-1 text-zinc-400">{binding.source_kind.replaceAll("_", " ")}</span>}<span className="block font-mono break-all text-zinc-500">→ {binding.target_port}</span></li>;
    })}</ul> : <p className="text-xs leading-5 text-zinc-400">No resolved port connections. Runtime choices may still be shown as step dependencies.</p>}</Section>
  </>;
}

function PlanInspector({ plan, instance, pending, canvasNode }: { plan: GraphDocument; instance?: GraphInstance; pending?: PendingRepeat; canvasNode?: CanvasStep }) {
  if (pending) return <>
    <InspectorHeading label="Pending repeat" title={pending.path} state="unexpanded" />
    <p className="fx-detail-copy">This repeat needs a runtime value before its individual steps can be expanded.</p>
    <dl className="mt-5 space-y-2 text-xs"><div>Maximum items: <strong>{pending.max}</strong></div><div>Phase: <strong>{pending.phase}</strong></div><div>Upper estimate: <strong>{money(pending.high_usd)}</strong></div></dl>
  </>;
  if (!instance) return <>
    <InspectorHeading label="Plan overview" title={plan.workflow.title} />
    {plan.workflow.description && <p className="fx-detail-copy">{plan.workflow.description}</p>}
    <p className="fx-detail-copy">Select a step to inspect its planned values, routes, identity, and dependencies.</p>
    <Section title="Estimated cost"><p className="font-mono text-sm">{money(plan.estimate.low_usd)} – {money(plan.estimate.high_usd)}</p><p className="mt-2 text-xs text-zinc-500">Budget ceiling: {plan.estimate.ceiling_usd === null ? "Not set" : money(plan.estimate.ceiling_usd)}</p></Section>
    {(plan.problems?.length ?? 0) > 0 && <Section title="Plan problems"><ul className="space-y-3">{plan.problems!.map((problem, index) => <li key={index} className="rounded-lg border border-red-200 bg-red-50 p-3 text-xs leading-5 text-red-800"><span className="block font-mono">{problem.where}</span>{problem.message}</li>)}</ul></Section>}
    <Section title="Pending repeats">{plan.pending.length ? <div className="space-y-2">{plan.pending.map((item) => {
      const id = `pending:${item.path}`;
      const content = <><span className="block font-mono">{item.path}</span><span className="mt-1 block text-zinc-500">Up to {item.max} items · phase {item.phase}</span></>;
      const style = "block w-full rounded-lg border border-dashed border-zinc-300 p-3 text-left text-xs";
      return controller.getSnapshot().graph.nodes.some((node) => node.id === id) ? <button key={item.path} onClick={() => controller.select(id)} className={style}>{content}</button> : <div key={item.path} className={style}>{content}</div>;
    })}</div> : <p className="text-xs text-zinc-400">No unexpanded repeats.</p>}</Section>
    {plan.inputs && Object.keys(plan.inputs).length > 0 && <Section title="Recorded workflow inputs"><JsonValue value={plan.inputs} /></Section>}
    {plan.workflow.file && <Section title="Source"><p className="font-mono text-xs break-all text-zinc-500">{plan.workflow.file}</p></Section>}
  </>;
  const meta = plan.steps?.[instance.step];
  const inputValues = Object.entries(instance.with).filter(([name]) => !canvasNode?.ports?.settings.some((setting) => setting.name === name));
  return <>
    <InspectorHeading label="Planned step" title={meta?.title || instance.path} code={instance.id} state={instance.state} />
    {meta?.description && <p className="fx-detail-copy">{meta.description}</p>}
    {instance.reason && <p className="mt-4 rounded-lg border border-amber-200 bg-amber-50 p-3 text-xs leading-5 text-amber-900">{instance.reason}</p>}
    <Section title="Node type"><p className="break-all font-mono text-xs text-zinc-600">{instance.uses}</p></Section>
    <Section title="Step estimate"><p className="font-mono text-sm">{money(instance.price.low_usd)} – {money(instance.price.high_usd)}</p><p className="mt-2 text-xs text-zinc-500">Phase {instance.phase} · take {instance.take.join(".")}{instance.key !== null ? ` · key ${instance.key}` : ""}</p></Section>
    <PortSections node={canvasNode} bindings={instance.interface_bindings ?? instance.bindings} values={instance.with} />
    <Section title="Planned input values">{inputValues.length ? <div className="space-y-3">{inputValues.map(([name, value]) => <div key={name}><p className="mb-1.5 font-mono text-[11px] text-zinc-500">{name}</p><JsonValue value={value} /></div>)}</div> : <p className="text-xs text-zinc-400">No input values.</p>}</Section>
    {instance.waiting_on.length > 0 && <div className="mt-4 rounded-lg border border-amber-200 bg-amber-50 p-3 text-xs leading-5 text-amber-900">Some values and the step identity are waiting for upstream results.</div>}
    <References title="Reads from" ids={instance.reads} /><References title="Required steps" ids={instance.needs} /><References title="Waiting for values" ids={instance.waiting_on} />
    <Section title="Routes">{Object.entries(instance.routes).length ? <div className="space-y-3">{Object.entries(instance.routes).map(([capability, route]) => <div key={capability}><p className="text-xs text-zinc-500">{capability}</p><p className="mt-1 font-mono text-xs break-all">{route.route}</p><details className="mt-1 text-[10px] text-zinc-400"><summary className="cursor-pointer">Route fingerprint</summary><p className="mt-1 font-mono break-all">{route.fingerprint}</p></details></div>)}</div> : <p className="text-xs text-zinc-400">No provider routes.</p>}</Section>
    <Section title="Identities"><dl className="space-y-3 text-xs"><div><dt className="text-zinc-500">Step</dt><dd className="mt-1 font-mono break-all">{instance.identity ?? "Not known yet"}</dd></div><div><dt className="text-zinc-500">Type</dt><dd className="mt-1 font-mono break-all">{instance.type ?? "Unresolved"}</dd></div></dl></Section>
    {instance.judges && <References title="Judges" ids={[instance.judges]} />}{instance.judged_by.length > 0 && <References title="Judged by" ids={instance.judged_by} />}
  </>;
}

function RunInspector({ run, node, canvasNode }: { run: ViewerRun; node?: ViewerRun["nodes"][number]; canvasNode?: CanvasStep }) {
  if (!node) return <>
    <InspectorHeading label="Run overview" title={run.workflow.title} />{run.workflow.description && <p className="fx-detail-copy">{run.workflow.description}</p>}
    <p className="fx-detail-copy">Select a step to inspect its recorded values and intermediate artifacts.</p>
    <Section title="Workflow outputs"><ValueList values={run.outputs} artifacts={run.artifacts} empty="No workflow outputs were recorded." /></Section><Section title="Workflow inputs"><ValueList values={run.inputs} artifacts={run.artifacts} empty="No workflow inputs were recorded." /></Section>
  </>;
  return <>
    <InspectorHeading label="Step" title={node.title || node.path} code={node.id} state={node.state} />
    <Section title="Node type"><p className="break-all font-mono text-xs text-zinc-600">{node.uses ?? "Not recorded"}</p></Section>
    <p className="fx-detail-copy">Cache: {node.cache ?? "Not recorded"} · Duration: {node.duration_ms === null ? "Not recorded" : `${(node.duration_ms / 1000).toLocaleString()} s`}</p>
    {node.error && <p role="alert" className="mt-4 rounded-lg border border-red-200 bg-red-50 p-3 text-xs leading-5 whitespace-pre-wrap text-red-800">{node.error}</p>}
    <PortSections node={canvasNode} bindings={node.interface_bindings ?? node.bindings} values={node.with} />
    <References title="Reads from" ids={node.reads} />{node.needs && <References title="Required steps" ids={node.needs} />}{node.judges && <References title="Judges" ids={[node.judges]} />}
    <Section title="Recorded input values"><ValueList values={Object.fromEntries(Object.entries(node.with).filter(([name]) => !canvasNode?.ports?.settings.some((setting) => setting.name === name)))} artifacts={run.artifacts} empty="No input values were recorded for this step." /></Section><Section title="Outputs"><ValueList values={node.outputs} artifacts={run.artifacts} empty="No output values were recorded for this step." /></Section>
  </>;
}

function ScopeInspector({ node, active = false }: { node: CanvasStep; active?: boolean }) {
  return <>
    <InspectorHeading label={active ? "Workflow overview" : node.kind === "workflow" ? "Imported workflow" : "Workflow interface"} title={node.title} code={node.path} state={node.state} />
    <p className="fx-detail-code">{node.subtitle}</p>
    {node.kind === "workflow" && <>
      <p className="fx-detail-copy">{node.child_count ?? 0} internal steps{node.failure_count ? ` · ${node.failure_count} failed` : ""}</p>
      {!active && <button onClick={() => node.scope_id && controller.openScope(node.scope_id)} className="fx-action mt-2">Open workflow →</button>}
    </>}
    {node.ports && <>
      <Section title="Input ports"><PortList ports={node.ports.inputs} /></Section>
      <Section title="Output ports"><PortList ports={node.ports.outputs} /></Section>
    </>}
    {!!node.errors?.length && <Section title="Internal failures"><ul className="space-y-3">{node.errors.map((error) => <li key={error.id} className="rounded-lg border border-red-200 bg-red-50 p-3 text-xs leading-5 text-red-800"><p className="mb-1 font-medium break-all">{error.title}</p><p className="whitespace-pre-wrap break-words">{error.message}</p></li>)}</ul></Section>}
  </>;
}

function RunPendingInspector({ node }: { node: CanvasStep }) {
  return <>
    <InspectorHeading label="Pending repeat" title={node.title} code={node.source_id} state={node.state} />
    <p className="fx-detail-copy">{node.subtitle}</p>
  </>;
}

function App() {
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  useEffect(() => { void controller.refresh(); return () => controller.dispose(); }, []);
  const { view, graph, selected, loading, error, updated, scope, breadcrumbs, scopeNode } = state;
  const plan = view?.kind === "fx-graph-v1" ? view : null;
  const run = view?.kind === "fx-viewer-run-v1" ? view : null;
  const selectedNode = graph.nodes.find((node) => node.id === selected);
  const instance = plan?.instances.find((node) => !selectedNode?.pending && node.id === selectedNode?.source_id);
  const pending = plan?.pending.find((node) => selectedNode?.pending && node.path === selectedNode.source_id);
  const runNode = run?.nodes.find((node) => node.id === selectedNode?.source_id);
  const back = controller.back;
  const refresh = <button onClick={() => void controller.refresh()} disabled={loading} className="fx-action"><RefreshCw {...iconProps} />{loading ? "Loading…" : "Refresh"}</button>;
  return <div className="fx-viewer">
    {error && <p role="alert" className="fx-error">{error}{view && " Showing the last loaded data."}</p>}
    {!view ? <main className="fx-unavailable"><h2>{loading ? "Opening the workflow…" : "Workflow unavailable"}</h2><p>{loading ? "Reading the selected graph or run." : "Check that the local viewer server is running."}</p>{!loading && <button onClick={() => void controller.refresh()} className="fx-action">Try again</button>}</main>
      : <div className="fx-workspace">
        <aside aria-label="Workflow outline" className="fx-outline">
          <div className="fx-workflow-heading">
            <h1>{view.workflow.title || "Workflow viewer"}</h1>
            <div className="fx-workflow-context"><span>{plan ? "Static plan" : "Run"}</span>{(run?.stand_in || plan?.stand_in) && <span className="fx-badge">Stand-in</span>}<span className="fx-readonly">Read only</span></div>
            {run && <code className="fx-run-name">{run.run_name}</code>}
          </div>
          <dl className="fx-run-summary">
            {run && <><div><dt>State</dt><dd><Status state={run.state} /></dd></div><div><dt>Spend</dt><dd className="font-mono">{money(run.charged_usd)}</dd></div></>}
            {plan && <div><dt>Expanded</dt><dd>{plan.instances.length} steps</dd></div>}
            <div><dt>Estimate</dt><dd className="font-mono">{view.estimate ? `${money(view.estimate.low_usd)} – ${money(view.estimate.high_usd)}` : "Not recorded"}</dd></div>
          </dl>
          <nav aria-label="Workflow steps" className="fx-step-nav">
            <button onClick={() => controller.select(null)} aria-current={selected === null ? "page" : undefined} className="fx-overview"><LayoutGrid {...iconProps} />{scopeNode ? "Workflow overview" : plan ? "Plan overview" : "Run overview"}</button>
            <div className="fx-list-heading"><span>Steps</span><span>{graph.nodes.length}</span></div>
            <div>{graph.nodes.map((node) => <button key={node.id} onClick={() => controller.select(node.id)} aria-current={selected === node.id ? "page" : undefined} aria-label={`${node.title}, ${node.state}, ${node.source_id}`} title={`${node.title} · ${node.state}\n${node.source_id}`} className="fx-step-row">
              {node.kind === "workflow" ? <Workflow {...iconProps} /> : <Square {...iconProps} />}
              <span className="fx-step-label"><span>{node.title}</span><code>{node.source_id}</code></span>
              <span className="fx-state-dot" data-state={node.state} aria-hidden="true" />
            </button>)}</div>
          </nav>
          {run && <div className="fx-run-controls">{refresh}<span className="fx-updated">{updated ? `Updated ${updated.toLocaleTimeString()}` : ""}</span></div>}
        </aside>
        <main aria-label="Workflow canvas" className="fx-canvas-pane">
          {!!view.scopes?.length && <nav aria-label="Workflow location" className="fx-location">
            <button onClick={back} disabled={scope === null} aria-label="Back to parent workflow" className="fx-back"><ArrowLeft {...iconProps} />Back</button>
            <div className="fx-breadcrumbs">{breadcrumbs.map((crumb, index) => <span key={crumb.id ?? "root"}>{index > 0 && <span aria-hidden="true" className="fx-breadcrumb-separator">/</span>}<button onClick={() => controller.goToScope(crumb.id)} title={crumb.id ?? crumb.title} aria-current={index === breadcrumbs.length - 1 ? "page" : undefined}>{crumb.title}</button></span>)}</div>
          </nav>}
          <div className="fx-canvas-surface"><WorkflowCanvas graph={graph} selected={selected} scope={scope} onSelect={controller.select} onOpenScope={controller.openScope} onBack={back} /></div>
          <div className="fx-canvas-context" title={`${view.workflow.title}${run ? ` · Run ${run.run_name}` : " · Static plan"} · Read only`}>{run ? refresh : <span className="fx-badge">Static plan · Read only</span>}</div>
        </main>
        <aside aria-label="Step details" className="fx-inspector">
          <div className="fx-panel-heading"><span>Inspector</span><span className="fx-panel-hint">{selectedNode?.kind ? "Workflow" : selectedNode ? "Step" : "Overview"}</span></div>
          <div className="fx-inspector-content">
            {run?.warnings.length ? <details className="fx-notice"><summary>{run.warnings.length} record {run.warnings.length === 1 ? "notice" : "notices"}</summary><ul>{run.warnings.map((warning, index) => <li key={index}>{warning}</li>)}</ul></details> : null}
            {selectedNode?.kind ? <ScopeInspector node={selectedNode} /> : !selectedNode && scopeNode ? <ScopeInspector node={scopeNode} active /> : plan ? <PlanInspector plan={plan} instance={instance} pending={pending} canvasNode={selectedNode} /> : run && selectedNode?.pending ? <RunPendingInspector node={selectedNode} /> : run ? <RunInspector run={run} node={runNode} canvasNode={selectedNode} /> : null}
          </div>
        </aside>
      </div>}
  </div>;
}

createRoot(document.getElementById("root")!).render(<StrictMode><App /></StrictMode>);
