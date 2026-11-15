import { useEffect, useRef } from "react";
import { renderNoteContent, type WorkflowNote } from "@grida/fx-web";

/** React mounts the same literal/CommonMark renderer used by the canvas and static viewer. */
export function NoteContent({ note }: { note: WorkflowNote }) {
  const target = useRef<HTMLDivElement>(null);
  const { format, text, diagnostic, withheld } = note;
  useEffect(() => {
    const root = target.current;
    if (!root) return;
    root.replaceChildren(renderNoteContent({ format, text, diagnostic, withheld }, root.ownerDocument));
    return () => root.replaceChildren();
  }, [format, text, diagnostic, withheld]);
  return <div ref={target} />;
}
