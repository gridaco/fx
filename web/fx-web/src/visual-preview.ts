import { createElement, Maximize, Minimize } from "lucide";

/** Shared fullscreen behavior for visual previews whose renderer has no native controls. */
export class VisualPreviewController {
  private readonly abort = new AbortController();
  private readonly button = document.createElement("button");
  private pending = false;
  private disposed = false;

  constructor(private readonly root: HTMLElement) {
    root.classList.add("fx-media-preview");
    this.button.type = "button";
    this.button.className = "fx-media-fullscreen";
    this.button.addEventListener("click", () => this.toggle(), { signal: this.abort.signal });
    root.append(this.button);
    root.addEventListener("keydown", event => {
      if (event.defaultPrevented || event.repeat || event.metaKey || event.ctrlKey || event.altKey) return;
      if (event.key === "Escape" && this.fullscreened) {
        event.preventDefault(); event.stopPropagation(); void this.exit();
      } else if (event.key.toLowerCase() === "f" && !(event.target as Element).closest("input, textarea, select, [contenteditable]")) {
        event.preventDefault(); event.stopPropagation(); this.toggle();
      }
    }, { capture: true, signal: this.abort.signal });
    root.ownerDocument.addEventListener("fullscreenchange", () => this.render(), { signal: this.abort.signal });
    this.render();
  }

  private get fullscreened(): boolean { return this.root.ownerDocument.fullscreenElement === this.root; }

  private toggle(): void {
    if (this.disposed || !this.root.ownerDocument.fullscreenEnabled || typeof this.root.requestFullscreen !== "function") return;
    if (this.fullscreened) { void this.exit(); return; }
    if (this.pending) return;
    this.pending = true;
    void this.root.requestFullscreen().then(() => {
      // A removed/replaced preview must not enter fullscreen after its renderer is gone.
      if (this.disposed) return this.exit();
    }).catch(() => {}).finally(() => { this.pending = false; });
  }

  private async exit(): Promise<void> {
    if (this.fullscreened) await this.root.ownerDocument.exitFullscreen().catch(() => {});
  }

  private render(): void {
    this.button.hidden = !this.root.ownerDocument.fullscreenEnabled || typeof this.root.requestFullscreen !== "function";
    const label = this.fullscreened ? "Exit full screen" : "Full screen";
    if (this.button.getAttribute("aria-label") === label) return;
    this.button.setAttribute("aria-label", label); this.button.title = label;
    this.button.replaceChildren(createElement(this.fullscreened ? Minimize : Maximize, { width: 14, height: 14, "aria-hidden": "true" }));
  }

  dispose(): void {
    this.disposed = true;
    this.abort.abort();
    this.button.remove();
    void this.exit();
  }
}
