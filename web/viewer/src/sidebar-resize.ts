/** Owns desktop sidebar resizing independently of React and workflow state. */
export class SidebarResize {
  private readonly abort = new AbortController();
  private active: { handle: HTMLElement; pointer: number; x: number; width: number; side: "left" | "right" } | null = null;

  constructor(private readonly workspace: HTMLElement) {
    for (const side of ["left", "right"] as const) {
      const handle = workspace.querySelector<HTMLElement>(`[data-resize="${side}"]`)!;
      const panel = workspace.querySelector<HTMLElement>(side === "left" ? ".fx-outline" : ".fx-inspector")!;
      const update = (width: number) => {
        const other = workspace.querySelector<HTMLElement>(side === "left" ? ".fx-inspector" : ".fx-outline")!;
        const minimum = side === "left" ? 180 : 240;
        const maximum = Math.max(minimum, Math.min(side === "left" ? 420 : 560, workspace.clientWidth - other.getBoundingClientRect().width - 320));
        const value = Math.round(Math.max(minimum, Math.min(maximum, width)));
        workspace.style.setProperty(`--fx-${side}-width`, `${value}px`);
        handle.setAttribute("aria-valuenow", String(value));
        handle.setAttribute("aria-valuemax", String(Math.round(maximum)));
      };
      handle.addEventListener("pointerdown", (event) => {
        if (event.button !== 0 || this.active) return;
        event.preventDefault();
        this.active = { handle, pointer: event.pointerId, x: event.clientX, width: panel.getBoundingClientRect().width, side };
        handle.setPointerCapture(event.pointerId);
        workspace.classList.add("fx-resizing");
      }, { signal: this.abort.signal });
      handle.addEventListener("pointermove", (event) => {
        const active = this.active;
        if (active?.handle !== handle || active.pointer !== event.pointerId) return;
        update(active.width + (event.clientX - active.x) * (side === "left" ? 1 : -1));
      }, { signal: this.abort.signal });
      const stop = (event: PointerEvent) => {
        if (this.active?.handle !== handle || this.active.pointer !== event.pointerId) return;
        this.active = null;
        workspace.classList.remove("fx-resizing");
        if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
      };
      for (const event of ["pointerup", "pointercancel", "lostpointercapture"] as const) handle.addEventListener(event, stop, { signal: this.abort.signal });
      handle.addEventListener("keydown", (event) => {
        if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
        event.preventDefault();
        update(panel.getBoundingClientRect().width + (event.key === "ArrowRight" ? 1 : -1) * (side === "left" ? 1 : -1) * (event.shiftKey ? 40 : 10));
      }, { signal: this.abort.signal });
      handle.addEventListener("dblclick", () => update(side === "left" ? 212 : 304), { signal: this.abort.signal });
    }
  }

  dispose() {
    this.abort.abort();
    if (this.active?.handle.hasPointerCapture(this.active.pointer)) this.active.handle.releasePointerCapture(this.active.pointer);
    this.active = null;
    this.workspace.classList.remove("fx-resizing");
  }
}
