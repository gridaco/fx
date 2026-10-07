import { useEffect, useRef } from "react";
import { CanvasController, type CanvasGraph } from "@grida/fx-web";
import "@grida/fx-web/canvas.css";

/** React only mounts the reusable DOM controller and forwards presentation props. */
export function WorkflowCanvas({ graph, selected, scope = null, onSelect, onOpenScope = () => {}, onBack = () => {} }: {
  graph: CanvasGraph;
  selected: string | null;
  scope?: string | null;
  onSelect: (id: string | null) => void;
  onOpenScope?: (id: string) => void;
  onBack?: () => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const controller = useRef<CanvasController | null>(null);
  const select = useRef(onSelect);
  const openScope = useRef(onOpenScope);
  const back = useRef(onBack);
  useEffect(() => { select.current = onSelect; openScope.current = onOpenScope; back.current = onBack; }, [onSelect, onOpenScope, onBack]);
  useEffect(() => {
    if (!container.current) return;
    controller.current = new CanvasController(container.current, (id) => select.current(id), (id) => openScope.current(id), () => back.current());
    return () => { controller.current?.dispose(); controller.current = null; };
  }, []);
  useEffect(() => { controller.current?.setGraph(graph, scope); }, [graph, scope]);
  useEffect(() => { controller.current?.setSelection(selected); }, [selected, graph]);
  return <div ref={container} className="h-full w-full" />;
}
