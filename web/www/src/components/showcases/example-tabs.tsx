"use client";

import { useEffect, useRef } from "react";
import { Copy } from "lucide-react";
import { ExampleTabsController } from "./example-tabs-controller";
import { AgentPrompt } from "./agent-prompt";
import type { AgentPromptPart } from "../../lib/agent-prompt";

export type ExamplePane = { label: string; language: string; code: string; note?: string; prompt?: readonly AgentPromptPart[] };

export function ExampleTabs({ id, panes }: { id: string; panes: ExamplePane[] }) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = new ExampleTabsController(root.current);
    return () => controller.dispose();
  }, []);

  return <div ref={root} className="mt-6 min-w-0">
    <div role="tablist" aria-label="Usage examples" className="flex gap-6 overflow-x-auto border-b">
      {panes.map((pane, index) => <button key={pane.label} role="tab" id={`${id}-tab-${index}`} aria-controls={`${id}-panel-${index}`} aria-selected={index === 0} tabIndex={index === 0 ? 0 : -1} className="shrink-0 border-b-2 border-transparent pb-3 text-sm text-muted-foreground aria-selected:border-foreground aria-selected:text-foreground">{pane.label}</button>)}
    </div>
    <div className="grid">
      {panes.map((pane, index) => <div key={pane.label} role="tabpanel" id={`${id}-panel-${index}`} aria-labelledby={`${id}-tab-${index}`} aria-hidden={index !== 0} inert={index !== 0} style={{ visibility: index === 0 ? "visible" : "hidden" }} className="col-start-1 row-start-1 min-w-0 pt-4">
        {pane.prompt ? <AgentPrompt parts={pane.prompt} /> : <div className="overflow-hidden rounded-lg border bg-muted/40">
          <div className="flex h-10 items-center justify-between border-b px-4 text-xs text-muted-foreground">
            <span>{pane.language}</span>
            <button type="button" data-copy-example className="inline-flex items-center gap-1.5"><Copy size={13} aria-hidden="true" /><span data-copy-label>Copy</span></button>
          </div>
          <pre className="overflow-x-auto p-4 text-[13px] leading-6"><code data-example-code className={pane.language === "Prompt" ? "block max-w-2xl font-sans whitespace-pre-wrap text-sm leading-7" : undefined}>{pane.code}</code></pre>
        </div>}
        {pane.note ? <p className="mt-3 max-w-2xl text-xs leading-6 text-muted-foreground">{pane.note}</p> : null}
      </div>)}
    </div>
  </div>;
}
