import { useEffect, useImperativeHandle, useRef, type Ref } from "react";
import { CanvasController, type CanvasGraph, type CanvasInset, type LayoutReport } from "@grida/fx-web";
import "@grida/fx-web/canvas.css";

/** What a host may ask of the canvas besides its props. */
export interface WorkflowCanvasHandle {
  reveal(id: string, inset?: CanvasInset): void;
  /** Apply imperative presentation before acknowledging an external page request. */
  present(graph: CanvasGraph, layout: LayoutReport | null, selected: string | null, scope: string | null, focus: string | "fit" | null, inset?: CanvasInset): void;
}

/** React only mounts the reusable DOM controller and forwards presentation props. */
export function WorkflowCanvas({ graph, layout = null, selected, scope = null, onSelect, onOpenScope = () => {}, onBack = () => {}, ref }: {
  graph: CanvasGraph;
  layout?: LayoutReport | null;
  selected: string | null;
  scope?: string | null;
  onSelect: (id: string | null) => void;
  onOpenScope?: (id: string) => void;
  onBack?: () => void;
  ref?: Ref<WorkflowCanvasHandle>;
}) {
  const container = useRef<HTMLDivElement>(null);
  const controller = useRef<CanvasController | null>(null);
  const select = useRef(onSelect);
  const openScope = useRef(onOpenScope);
  const back = useRef(onBack);
  useImperativeHandle(ref, () => ({
    reveal: (id, inset) => controller.current?.reveal(id, inset),
    present: (graph, layout, selected, scope, focus, inset) => {
      if (!controller.current) throw new Error("The workflow canvas is not mounted.");
      controller.current.setGraph(graph, scope, layout);
      controller.current.setSelection(selected);
      if (focus === "fit") controller.current.fit();
      else if (focus !== null) controller.current.reveal(focus, inset);
    },
  }), []);
  useEffect(() => { select.current = onSelect; openScope.current = onOpenScope; back.current = onBack; }, [onSelect, onOpenScope, onBack]);
  useEffect(() => {
    if (!container.current) return;
    controller.current = new CanvasController(container.current, (id) => select.current(id), (id) => openScope.current(id), () => back.current());
    return () => { controller.current?.dispose(); controller.current = null; };
  }, []);
  useEffect(() => { controller.current?.setGraph(graph, scope, layout); }, [graph, scope, layout]);
  useEffect(() => { controller.current?.setSelection(selected); }, [selected, graph]);
  return <div ref={container} className="h-full w-full" />;
}
