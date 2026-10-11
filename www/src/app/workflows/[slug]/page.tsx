import Link from "next/link";
import { notFound } from "next/navigation";
import { ArrowLeft, ArrowRight, ArrowDownToLine, ArrowUpRight } from "lucide-react";
import { pageMetadata } from "../../../lib/metadata";
import { workflowAsset, workflows } from "../../../lib/workflows";
import { workflowDetails, assetExample } from "../../../lib/workflow-details";
import { WorkflowDemo } from "../../../components/showcases/workflow-demo";
import { agentPromptText } from "../../../lib/agent-prompt";
import { ExampleTabs } from "../../../components/showcases/example-tabs";
import { RecordedCanvas } from "../../../components/showcases/recorded-canvas";
import { workflowRecordings } from "../../../lib/showcases/catalog";
import { site } from "../../../../site.config";

type Props = { params: Promise<{ slug: string }> };

export const dynamicParams = false;

export function generateStaticParams() {
  return workflows.map(({ slug }) => ({ slug }));
}

export async function generateMetadata({ params }: Props) {
  const { slug } = await params;
  const workflow = workflows.find((entry) => entry.slug === slug);
  if (!workflow) notFound();
  return pageMetadata(`${workflow.title} — Grida FX — Grida`, workflow.description, `workflows/${workflow.slug}/`);
}

export default async function WorkflowPage({ params }: Props) {
  const { slug } = await params;
  const workflow = workflows.find((entry) => entry.slug === slug);
  if (!workflow) notFound();
  const base = workflowAsset(workflow, "").replace(/\/$/, "");
  const details = workflowDetails[workflow.slug];
  const usage = assetExample(workflow.slug, base);
  const related = workflows.find(({ slug }) => slug === details.related)!;
  const builder = workflow.slug === "terrain" || workflow.slug === "ui-kit";
  const target = builder ? "./workflow.py:build" : `./workflows/${workflow.slug}.yaml`;
  const options = builder ? "" : " --inputs inputs.yaml";
  const prompt = details.agent.map((part) => {
    if (typeof part === "string") return part;
    const source = part.workflow ? workflows.find(({ slug }) => slug === part.workflow)! : workflow;
    const href = workflowAsset(source, part.path);
    return { label: part.label, href, image: href };
  });
  const panes = [
    { label: "For agents", language: "Prompt", code: agentPromptText(prompt), prompt },
    { label: builder ? "Specification" : "Inputs", language: builder ? "Design example" : "inputs.yaml", code: details.inputs, note: builder ? "This is an authoring specification for a Python builder, rather than CLI input fields." : "Input example for the custom workflow shown here. It requires that workflow's definition and node implementations." },
    { label: "CLI", language: "Shell", code: `# In your project, with the workflow and its custom nodes.\ngrida-fx plan ${target}${options}\n\n# Start the local service for the browser viewer.\ngrida-fx start --background\n\n# After reviewing the plan, choose a provider spending ceiling.\ngrida-fx run ${target}${options} --live --max-usd 10 --open`, note: "Command pattern for your own workflow project. These custom workflows are not installed by the FX package. Planning is offline; a live run uses your provider account." },
    { label: "Use the assets", language: usage.language, code: usage.code, note: "Consume the recorded files directly. No engine or provider call is needed to display them. Rendering excerpts assume your application's canvas or media setup." },
  ];

  return <main id="main" className="fx-site-container py-10 md:py-14">
    <Link href="/workflows/" className="inline-flex items-center gap-2 text-sm text-muted-foreground hover:text-foreground"><ArrowLeft size={15} aria-hidden="true" /> Workflows</Link>
    <header className="mt-7">
      <div className="max-w-2xl">
        <h1 className="text-3xl font-semibold tracking-tight sm:text-4xl">{workflow.title}</h1>
        <p className="mt-4 text-base leading-7 text-muted-foreground">{workflow.description}</p>
      </div>
    </header>
    <RecordedCanvas recordings={workflowRecordings(workflow.slug)}>
    <p className="mt-6 flex flex-wrap items-center gap-3 text-xs text-muted-foreground"><span>{workflow.input}</span><ArrowRight size={13} aria-hidden="true" /><span>{workflow.output}</span></p>
    <div className="mt-7 fx-demo"><WorkflowDemo slug={workflow.slug} assetBase={base} /></div>

    <section className="mt-14 grid gap-8 border-t pt-10 md:grid-cols-[1fr_2fr] md:gap-14" aria-labelledby="process-title">
      <div>
        <h2 id="process-title" className="text-xl font-semibold tracking-tight">How it comes together</h2>
        <p className="mt-4 max-w-sm text-sm leading-7 text-muted-foreground">{details.introduction}</p>
      </div>
      <ol className="grid gap-x-8 gap-y-7 sm:grid-cols-2">
        {details.stages.map((stage, index) => <li key={stage.title}>
          <p className="text-xs text-muted-foreground">0{index + 1}</p>
          <h3 className="mt-2 text-sm font-medium">{stage.title}</h3>
          <p className="mt-2 text-sm leading-6 text-muted-foreground">{stage.description}</p>
        </li>)}
      </ol>
    </section>

    <section className="mt-14 grid gap-8 border-t pt-10 md:grid-cols-[1fr_2fr] md:gap-14" aria-labelledby="outputs-title">
      <div>
        <h2 id="outputs-title" className="text-xl font-semibold tracking-tight">Take it from here</h2>
        <p className="mt-4 max-w-sm text-sm leading-7 text-muted-foreground">Real files from this run. Use them in your own renderer, prototype, or engine.</p>
      </div>
      <ul className="divide-y">
        {details.outputs.map((output) => <li key={output.path} className="py-4 first:pt-0">
          <a href={workflowAsset(workflow, output.path)} download className="group flex items-start justify-between gap-4">
            <div className="min-w-0"><p className="break-all font-mono text-xs">{output.path}</p><p className="mt-1.5 text-sm text-muted-foreground">{output.description}</p></div>
            <ArrowDownToLine size={15} className="mt-1 shrink-0 text-muted-foreground group-hover:text-foreground" aria-hidden="true" />
          </a>
        </li>)}
      </ul>
    </section>

    <section className="mt-14 grid gap-8 border-t pt-10 md:grid-cols-[1fr_2fr] md:gap-14" aria-labelledby="usage-title">
      <div>
        <h2 id="usage-title" className="text-xl font-semibold tracking-tight">Make it yours</h2>
        <p className="mt-4 max-w-sm text-sm leading-7 text-muted-foreground">Give your agent the brief, author a workflow, or start by using the finished assets.</p>
        <a href={site.guide_url} className="mt-5 inline-flex items-center gap-1.5 text-sm">FX guide <ArrowUpRight size={14} aria-hidden="true" /></a>
      </div>
      <div className="min-w-0 -mt-6"><ExampleTabs id={workflow.slug} panes={panes} /></div>
    </section>

    <section className="mt-14 grid gap-8 border-t pt-10 md:grid-cols-[1fr_2fr] md:gap-14" aria-labelledby="scope-title">
      <h2 id="scope-title" className="text-xl font-semibold tracking-tight">A good fit for</h2>
      <div>
        <ul className="space-y-2 text-sm leading-7">{details.works.map((item) => <li key={item}>{item}</li>)}</ul>
        <details className="mt-5 text-xs leading-6 text-muted-foreground">
          <summary className="cursor-pointer">Production notes</summary>
          <p className="mt-2">{workflow.note}</p>
          <a href={workflowAsset(workflow, "manifest.json")} className="mt-2 inline-flex items-center gap-1.5 underline underline-offset-4">Demo manifest <ArrowUpRight size={12} aria-hidden="true" /></a>
        </details>
      </div>
    </section>

    </RecordedCanvas>
    <Link href={`/workflows/${related.slug}/`} className="mt-14 flex items-center justify-between gap-6 rounded-xl border p-6">
      <div><p className="text-xs text-muted-foreground">Keep exploring</p><p className="mt-2 text-lg font-medium tracking-tight">{related.title}</p></div>
      <ArrowRight size={20} aria-hidden="true" />
    </Link>
  </main>;
}
