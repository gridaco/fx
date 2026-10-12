"use client";

import { useEffect, useRef } from "react";
import { Check, Copy } from "lucide-react";
import { GettingStartedController } from "./getting-started-controller";

const command = "npx grida fx init my-assets";
const agentCommand = "npx skills add gridaco/fx";

/** Static markup and a thin mount adapter; copying belongs to the controller. */
export function GettingStarted() {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = new GettingStartedController(root.current);
    return () => controller.dispose();
  }, []);

  return <div ref={root} className="min-w-0">
    <div className="flex h-11 min-w-0 items-center gap-2 rounded-full border bg-muted/30 pr-1.5 pl-5">
      <span className="font-mono text-[13px] text-muted-foreground select-none" aria-hidden="true">$</span>
      <code data-command className="min-w-0 flex-1 overflow-x-auto font-mono text-[13px] leading-5 whitespace-nowrap">{command}</code>
      <button type="button" data-copy aria-label="Copy command" title="Copy command"
        className="group flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full text-muted-foreground hover:bg-muted hover:text-foreground">
        <Copy size={14} aria-hidden="true" className="group-data-[copied=true]:hidden" />
        <Check size={14} aria-hidden="true" className="hidden group-data-[copied=true]:block" />
      </button>
    </div>
    <p className="mt-2 px-5 text-xs text-muted-foreground">
      Using an agent? <code className="font-mono text-foreground">{agentCommand}</code>
    </p>
    <span data-copy-status role="status" aria-live="polite" className="sr-only" />
  </div>;
}
