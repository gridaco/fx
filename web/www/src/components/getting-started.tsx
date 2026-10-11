"use client";

import { useEffect, useRef } from "react";
import { Check, ChevronDown, Copy } from "lucide-react";
import { GettingStartedController } from "./getting-started-controller";
import { installOptions } from "./install-options";

const audiences = [
  {
    id: "humans",
    label: "For humans",
    command: installOptions[0].command,
  },
  {
    id: "agents",
    label: "For agents",
    command: "npx skills add gridaco/fx",
  },
] as const;

/** Static markup and a thin mount adapter; interaction belongs to the controller. */
export function GettingStarted() {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = new GettingStartedController(root.current);
    return () => controller.dispose();
  }, []);

  return <div ref={root}>
    <div role="tablist" aria-label="Choose how to get started" className="mb-3 flex gap-6 text-xs">
      {audiences.map((audience) => <button key={audience.id} type="button" role="tab"
        id={`start-tab-${audience.id}`} data-audience={audience.id}
        aria-controls={`start-panel-${audience.id}`} aria-selected={audience.id === "humans"}
        tabIndex={audience.id === "humans" ? 0 : -1}
        className="cursor-pointer border-b border-transparent pb-2 text-muted-foreground aria-selected:border-foreground aria-selected:text-foreground">
        {audience.label}
      </button>)}
    </div>
    {audiences.map((audience) => <div key={audience.id} role="tabpanel"
      id={`start-panel-${audience.id}`} data-panel={audience.id}
      aria-labelledby={`start-tab-${audience.id}`} hidden={audience.id !== "humans"} tabIndex={0}>
      <div className="flex h-10 min-w-0 items-center gap-2 rounded-md border bg-muted/30 px-2.5">
        {audience.id === "humans" && <div className="relative shrink-0 border-r pr-2">
          <select data-install-option aria-label="Installation option" defaultValue={installOptions[0].id}
            className="cursor-pointer appearance-none bg-transparent py-1 pr-5 text-xs outline-offset-2">
            {installOptions.map((option) => <option key={option.id} value={option.id}>{option.label}</option>)}
          </select>
          <ChevronDown size={12} aria-hidden="true" className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 text-muted-foreground" />
        </div>}
        <span className="font-mono text-xs text-muted-foreground select-none" aria-hidden="true">$</span>
        <code data-command className="min-w-0 flex-1 overflow-x-auto font-mono text-xs leading-5 whitespace-nowrap">{audience.command}</code>
        <button type="button" data-copy aria-label="Copy setup command" title="Copy setup command"
          className="group flex size-6 shrink-0 cursor-pointer items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground">
          <Copy size={14} aria-hidden="true" className="group-data-[copied=true]:hidden" />
          <Check size={14} aria-hidden="true" className="hidden group-data-[copied=true]:block" />
        </button>
      </div>
    </div>)}
    <span data-copy-status role="status" aria-live="polite" className="mt-1 block min-h-4 text-left text-[11px] text-muted-foreground" />
  </div>;
}
