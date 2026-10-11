import { ArrowUp, Copy, FileText, Paperclip } from "lucide-react";
import { agentPromptText, type AgentPromptPart } from "../../lib/agent-prompt";

/** A copyable prompt, presented as a chat composer without a pretend chat service. */
export function AgentPrompt({ parts }: { parts: readonly AgentPromptPart[] }) {
  let number = 0;
  const references = parts.filter((part) => typeof part !== "string");
  return <>
    <div className="rounded-3xl bg-foreground p-5 text-background sm:p-6">
      <div className="whitespace-pre-wrap text-[15px] leading-8">
        {parts.map((part, index) => typeof part === "string" ? part : <a
          key={index}
          href={part.href}
          data-prompt-reference
          data-reference-label={part.label}
          target="_blank"
          rel="noopener noreferrer"
          aria-label={`View reference ${number + 1}: ${part.label}`}
          className="mx-1 inline-flex max-w-full items-center gap-2 rounded-lg bg-white/10 py-1 pl-1 pr-2.5 align-middle text-sm leading-6 transition-colors hover:bg-white/20"
        >
          {part.image ? <img src={part.image} alt="" loading="lazy" className="size-8 shrink-0 rounded-md bg-white/10 object-cover object-top" /> : <FileText size={18} className="ml-1 shrink-0" aria-hidden="true" />}
          <span className="flex size-4 shrink-0 items-center justify-center rounded-full bg-background text-[10px] font-semibold leading-none text-foreground">{++number}</span>
          <span>{part.label}</span>
        </a>)}
      </div>
      <code data-example-code hidden>{agentPromptText(parts)}</code>
      <div className="mt-5 flex items-center justify-between gap-3">
        <span className="inline-flex items-center gap-2 text-xs text-white/50"><Paperclip size={15} aria-hidden="true" />{references.length} {references.length === 1 ? "reference" : "references"}</span>
        <button type="button" data-copy-example data-copy-default="Copy prompt" className="group inline-flex items-center gap-2.5 rounded-full text-xs text-white/60 hover:text-white">
          <Copy size={14} aria-hidden="true" /><span data-copy-label aria-live="polite">Copy prompt</span>
          <span className="ml-1 flex size-9 items-center justify-center rounded-full bg-background text-foreground group-hover:bg-white/90"><ArrowUp size={18} aria-hidden="true" /></span>
        </button>
      </div>
    </div>
    <p className="mt-3 max-w-3xl text-xs leading-6 text-muted-foreground">Paste into your agent. Copy includes text and reference links; attach your own files to make it yours.</p>
  </>;
}
