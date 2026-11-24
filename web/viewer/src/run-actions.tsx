import { useEffect, useRef } from "react";
import { Pause, Play, RotateCcw, Square, X } from "lucide-react";
import { currentRunAction, requiresRunConfirmation, runActions, type RunAction, type RunActionsController, type RunActionsState } from "@grida/fx-web";

const iconProps = { size: 14, strokeWidth: 1.5, "aria-hidden": true } as const;
const actions = {
  pause: { label: "Pause", description: "Pause after the running steps finish.", icon: Pause },
  resume: { label: "Resume", description: "Pick up where you left off.", icon: Play },
  rerun: { label: "Rerun workflow", description: "Run the workflow again with the same inputs.", icon: RotateCcw },
  cancel: { label: "Cancel run", description: "Ask this run to stop.", icon: Square },
} satisfies Record<RunAction, { label: string; description: string; icon: typeof Pause }>;

function confirmation(action: RunAction, attempt: number): string {
  if (action === "rerun") {
    return `Run the workflow again with the same inputs as attempt ${attempt}. FX may reuse earlier work. The new attempt opens here when it starts, and both attempts are available in history.`;
  }
  if (action === "cancel") return `Stop attempt ${attempt}. Steps that are already running may take a moment to stop. You can still see the work that finished.`;
  return actions[action].description;
}

function ActionReceipt({ receipt }: { receipt: NonNullable<RunActionsState["receipt"]> }) {
  return <div className={receipt.outcome === "accepted" ? "fx-run-receipt" : "fx-detail-problem"}>
    <p role={receipt.outcome === "accepted" ? "status" : "alert"}>{receipt.message}</p>
    {receipt.cli_command && <code className="fx-run-command">{receipt.cli_command}</code>}
  </div>;
}

function RunActionDialog({ state, selection, controller, invocationId }: {
  state: RunActionsState;
  selection: NonNullable<RunActionsState["selection"]>;
  controller: RunActionsController;
  invocationId: string | null;
}) {
  const { control, error, checking, submitting, receipt } = state;
  const { action, attempt_number } = selection;
  const dialog = useRef<HTMLDialogElement>(null);
  const back = useRef<HTMLButtonElement>(null);
  const retry = receipt?.outcome === "unknown";
  const refused = receipt?.outcome === "refused";
  const stale = !receipt && !submitting && !checking && (error || !currentRunAction(control, selection, invocationId));
  useEffect(() => {
    const element = dialog.current;
    if (!element) return;
    element.showModal();
    return () => { element.close(); };
  }, []);
  useEffect(() => { back.current?.focus({ preventScroll: true }); }, [refused, retry]);
  return <dialog ref={dialog} className="fx-run-dialog" aria-labelledby="fx-run-dialog-title"
    aria-describedby="fx-run-dialog-description" data-fx-owns-keys
    onKeyDown={event => { if (event.key === "Escape") event.stopPropagation(); }}
    onCancel={event => { event.preventDefault(); controller.back(); }}>
    <h2 id="fx-run-dialog-title">{action === "rerun" ? "Rerun this workflow?" : "Cancel this run?"}</h2>
    <p id="fx-run-dialog-description">{confirmation(action, attempt_number)}</p>
    {receipt && <ActionReceipt receipt={receipt} />}
    {stale && <p role="alert" className="fx-detail-problem">This action isn’t available right now. Go back to check the run’s status.</p>}
    {submitting && <p role="status" className="fx-detail-meta">Sending request…</p>}
    <div className="fx-run-confirm-buttons">
      <button ref={back} type="button" className="fx-action" disabled={submitting} onClick={controller.back}>Back</button>
      {!refused && <button type="button" className="fx-action" data-destructive={action === "cancel"}
        disabled={submitting || checking || Boolean(stale)} onClick={() => void controller.submit(invocationId)}>
        {retry ? "Retry request" : actions[action].label}
      </button>}
    </div>
  </dialog>;
}

export function RunActionsPanel({ state, controller, invocationId, onClose }: {
  state: RunActionsState;
  controller: RunActionsController;
  invocationId: string | null;
  onClose: () => void;
}) {
  const { control, error, selection, checking, submitting, receipt, rerunUnconfirmed } = state;
  const heading = useRef<HTMLHeadingElement>(null);
  const opener = useRef<HTMLButtonElement>(null);
  const confirming = selection !== null && requiresRunConfirmation(selection.action);
  useEffect(() => {
    if (confirming) return;
    const button = opener.current;
    opener.current = null;
    (button?.isConnected && !button.disabled ? button : heading.current)?.focus({ preventScroll: true });
  }, [confirming]);
  const available = runActions.filter(action => control?.actions[action] || action === "rerun" && rerunUnconfirmed);
  const matches = invocationId !== null && control?.invocation_id === invocationId;
  return <aside className="fx-details fx-run-actions" id="fx-run-actions" aria-labelledby="fx-run-actions-title" data-fx-owns-keys
    onKeyDown={event => {
      if (event.key !== "Escape" || event.defaultPrevented || event.currentTarget.ownerDocument.fullscreenElement) return;
      event.preventDefault();
      event.stopPropagation();
      if (selection && !submitting) controller.back();
      else onClose();
    }}>
    <div className="fx-detail-heading">
      <div className="fx-detail-heading-row"><h2 ref={heading} tabIndex={-1} id="fx-run-actions-title">Run actions</h2></div>
      {control && <p className="fx-detail-meta">Attempt {selection?.attempt_number ?? control.attempt_number}</p>}
    </div>
    <button type="button" className="fx-details-close" aria-label="Close run actions" onClick={onClose}><X {...iconProps} /></button>
    {error && <p role="alert" className="fx-detail-problem">{error}</p>}
    {checking && <p role="status" className="fx-detail-copy">Checking the run’s status…</p>}
    {!checking && !error && !selection && !matches && available.length > 0 && <p role="status" className="fx-detail-copy">Waiting for the run to update…</p>}
    {receipt && !confirming && <ActionReceipt receipt={receipt} />}
    {selection && !confirming ? <div className="fx-run-inline">
      {submitting && <p role="status" className="fx-detail-meta">Sending request…</p>}
      {receipt?.outcome === "unknown" && <div className="fx-run-confirm-buttons">
        <button type="button" className="fx-action" disabled={submitting} onClick={controller.back}>Back</button>
        <button type="button" className="fx-action" disabled={submitting || checking} onClick={() => void controller.submit(invocationId)}>
          Retry request
        </button>
      </div>}
    </div> : available.length > 0 ? <ul className="fx-run-action-list">{available.map(action => {
      const { label, description, icon: Icon } = actions[action];
      const retryRerun = action === "rerun" && rerunUnconfirmed;
      return <li className="fx-run-action-row" key={action}>
        <p id={`fx-run-action-hint-${action}`}>{retryRerun ? "Check the previous rerun request." : description}</p>
        <button type="button" className="fx-action" disabled={checking || !retryRerun && (!matches || Boolean(error))} aria-describedby={`fx-run-action-hint-${action}`}
          data-destructive={action === "cancel"} onClick={event => {
            if (requiresRunConfirmation(action)) opener.current = event.currentTarget;
            controller.choose(action, invocationId);
          }}><Icon {...iconProps} />{label}</button>
      </li>;
    })}</ul> : !checking && !error && <p className="fx-detail-copy">No run actions are available right now.</p>}
    {confirming && selection && <RunActionDialog state={state} selection={selection} controller={controller} invocationId={invocationId} />}
  </aside>;
}
