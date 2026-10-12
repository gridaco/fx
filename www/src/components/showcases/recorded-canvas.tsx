"use client";

import { useEffect, useRef, type ReactNode } from "react";
import { X } from "lucide-react";
import { RecordedCanvasController, type Recording } from "./recorded-canvas-controller";
import "@grida/fx-web/canvas.css";
import styles from "./recorded-canvas.module.css";

/** React supplies the document; the static controller owns tabs, fetches, navigation and inspection. */
export function RecordedCanvas({ recordings, children }: { recordings: readonly Recording[]; children: ReactNode }) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = new RecordedCanvasController(root.current, recordings);
    return () => controller.dispose();
  }, [recordings]);
  return <div ref={root} className={styles.presentation}>
    <div className="fx-site-container">
      <div role="tablist" aria-label="Workflow presentation" className={styles.tabs}>
        <button role="tab" data-mode="overview" id="workflow-overview-tab" aria-controls="workflow-overview" aria-selected="true">Overview</button>
        <button role="tab" data-mode="graph" id="workflow-graph-tab" aria-controls="workflow-graph" aria-selected="false" tabIndex={-1}>Workflow</button>
      </div>
    </div>
    {/* The overview's content is padded; the recorded canvas spans the full width. */}
    <div role="tabpanel" id="workflow-overview" aria-labelledby="workflow-overview-tab" data-overview className="fx-site-container">{children}</div>
    <div role="tabpanel" id="workflow-graph" aria-labelledby="workflow-graph-tab" data-graph hidden className={styles.graphPanel}>
      <div className={`fx-site-container ${styles.recordingBar}`}>
        {recordings.length > 1 ? <label>Recording <select data-recording defaultValue={recordings[0]?.id}>{recordings.map((recording) => <option key={recording.id} value={recording.id}>{recording.label}</option>)}</select></label> : <span>{recordings[0]?.label ?? "Recording"}</span>}
        <span data-recording-summary>Saved execution · Read only</span>
      </div>
      <div className="fx-site-container"><p data-recording-status role="status" className={styles.message} hidden /></div>
      <div className={styles.workspace} data-workspace hidden>
        <div data-canvas className={styles.canvas} />
        <nav aria-label="Workflow location" data-breadcrumbs className={styles.breadcrumbs} hidden />
        <aside aria-label="Recorded step details" data-inspector className={styles.inspector} hidden>
          <button type="button" data-close-inspector aria-label="Close step details" className={styles.close}><X size={15} aria-hidden="true" /></button>
          <div data-inspector-body className={styles.inspectorBody} />
        </aside>
      </div>
      <div className="fx-site-container"><p data-recording-disclosure className={styles.disclosure} hidden /></div>
    </div>
  </div>;
}
