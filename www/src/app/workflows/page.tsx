import Link from "next/link";
import { ArrowRight } from "lucide-react";
import { pageMetadata } from "../../lib/metadata";
import { workflows } from "../../lib/workflows";
import { workflowCover } from "../../lib/showcases/covers";
import { WorkflowCover } from "../../components/showcases/workflow-cover";

const description = "Explore reusable Grida FX workflows for parallax backgrounds, illustrated storyworlds, transparent sprite loops, animated pets, facial patches, rigged 3D characters, 47-tile terrain atlases, and nine-slice UI kits.";

export const metadata = pageMetadata("Workflows — Grida FX — Grida", description, "workflows");

export default function WorkflowsPage() {
  return <main id="main" className="fx-site-container py-12 md:py-16">
    <div className="flex items-center justify-between gap-4">
      <h1 className="text-3xl font-semibold tracking-tight sm:text-4xl">Workflows</h1>
    </div>
    <ul className="mt-8 space-y-12" aria-label="Workflow showcases">
      {workflows.map((workflow, index) => <li key={workflow.slug}>
        <article className="grid items-center gap-5 md:grid-cols-[1.7fr_1fr] md:gap-10">
          <Link href={`/workflows/${workflow.slug}`} className="block aspect-[8/5] overflow-hidden rounded-xl bg-muted" aria-label={`Explore ${workflow.title}`}>
            <WorkflowCover media={workflowCover(workflow.slug)} title={workflow.title} eager={index === 0} />
          </Link>
          <div>
            <h2 className="text-2xl font-semibold tracking-tight"><Link href={`/workflows/${workflow.slug}`}>{workflow.title}</Link></h2>
            <p className="mt-3 w-[85%] max-w-sm text-sm leading-6 text-muted-foreground md:w-[70%]">{workflow.description}</p>
            <Link href={`/workflows/${workflow.slug}`} className="mt-5 inline-flex items-center gap-2 text-sm font-medium">Explore <ArrowRight size={15} aria-hidden="true" /></Link>
          </div>
        </article>
      </li>)}
    </ul>
  </main>;
}
