/** Hover and keyboard previews retain the reference link's normal click behavior. */
export class ReferencePreviewController {
  private readonly listeners = new AbortController();
  private active: { reference: HTMLAnchorElement; preview: HTMLElement; dismiss: () => void } | null = null;

  constructor(root: HTMLElement) {
    const options = { signal: this.listeners.signal };
    const view = root.ownerDocument.defaultView;
    for (const reference of root.querySelectorAll<HTMLAnchorElement>("[data-prompt-reference]")) {
      const preview = reference.querySelector<HTMLElement>("[data-reference-preview]");
      if (!preview) continue;
      let hovered = false, focused = false, dismissed = false;
      const update = () => {
        if ((hovered || focused) && !dismissed) {
          this.hide();
          this.active = { reference, preview, dismiss: () => { dismissed = true; update(); } };
          preview.hidden = false;
          this.position();
        } else if (this.active?.preview === preview) this.hide();
      };
      reference.addEventListener("pointerenter", event => {
        if (event.pointerType === "touch") return;
        hovered = true; dismissed = false; update();
      }, options);
      reference.addEventListener("pointerleave", () => { hovered = false; update(); }, options);
      reference.addEventListener("focus", () => { focused = true; dismissed = false; update(); }, options);
      reference.addEventListener("blur", () => { focused = false; update(); }, options);
      preview.querySelector("img")?.addEventListener("load", () => {
        if (this.active?.preview === preview) this.position();
      }, options);
    }
    root.ownerDocument.addEventListener("keydown", event => {
      if (event.key === "Escape" && this.active) {
        event.preventDefault();
        this.active.dismiss();
      }
    }, options);
    view?.addEventListener("scroll", () => this.active?.dismiss(), { ...options, capture: true, passive: true });
    view?.addEventListener("resize", () => this.active?.dismiss(), options);
  }

  dispose() {
    this.listeners.abort();
    this.hide();
  }

  private hide() {
    if (this.active) this.active.preview.hidden = true;
    this.active = null;
  }

  private position() {
    if (!this.active) return;
    const { reference, preview } = this.active;
    const view = reference.ownerDocument.defaultView;
    if (!view) return;
    const anchor = reference.getBoundingClientRect(), card = preview.getBoundingClientRect();
    const inset = 12;
    const preferredTop = anchor.top >= card.height + inset ? anchor.top - card.height : anchor.bottom;
    preview.style.left = `${Math.max(inset, Math.min(anchor.left, view.innerWidth - card.width - inset))}px`;
    preview.style.top = `${Math.max(inset, Math.min(preferredTop, view.innerHeight - card.height - inset))}px`;
  }
}
