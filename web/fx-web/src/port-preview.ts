import { createElement, X } from "lucide";
import type { CanvasPort, CanvasStep } from "./graph";
import { displayPortName } from "./ports";
import type { PortImage } from "./port-values";
import { VisualPreviewController } from "./visual-preview";

export interface InspectedPort { node: string; name: string; direction: "input" | "output"; kind: CanvasPort["kind"] }
interface PortTarget extends InspectedPort { anchor: SVGGElement; port: CanvasPort; title: string }

/** One screen-space inspector shared by sockets. Opening it never changes graph geometry. */
export class PortPreview {
  private static nextId = 0;
  readonly element = document.createElement("div");
  private readonly heading = document.createElement("code");
  private readonly context = document.createElement("span");
  private readonly type = document.createElement("span");
  private readonly description = document.createElement("p");
  private readonly content = document.createElement("div");
  private readonly gallery = document.createElement("div");
  private readonly imageList = document.createElement("div");
  private readonly imageCount = document.createElement("span");
  private readonly body = document.createElement("pre");
  private readonly sources = document.createElement("div");
  private readonly hint = document.createElement("span");
  private readonly abort = new AbortController();
  private target: PortTarget | null = null;
  private pinned = false;
  private closing = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private visual: VisualPreviewController | null = null;
  private imageAbort: AbortController | null = null;

  get fullscreened() { return this.element.ownerDocument.fullscreenElement === this.gallery; }

  constructor(private readonly container: HTMLElement, private readonly onInspect: (port: InspectedPort | null) => void,
    private readonly covered: (element: Element) => boolean, private readonly canHover: () => boolean) {
    this.element.id = `fx-port-preview-${++PortPreview.nextId}`;
    this.element.className = "fx-port-preview";
    this.element.hidden = true;
    this.element.setAttribute("role", "region");
    const header = document.createElement("div"), labels = document.createElement("div"), footer = document.createElement("div");
    header.className = "fx-port-preview-header";
    labels.className = "fx-port-preview-labels";
    this.context.className = "fx-port-preview-context";
    this.type.className = "fx-port-preview-type";
    this.description.className = "fx-port-preview-description";
    this.description.hidden = true;
    this.description.tabIndex = 0;
    this.gallery.className = "fx-port-images";
    this.gallery.hidden = true;
    this.gallery.setAttribute("role", "group");
    this.gallery.setAttribute("aria-label", "Image preview");
    this.imageList.className = "fx-media-content fx-port-image-list";
    this.imageList.tabIndex = 0;
    this.imageList.setAttribute("aria-label", "Image previews");
    this.imageCount.className = "fx-port-image-count";
    this.gallery.append(this.imageList, this.imageCount);
    this.gallery.addEventListener("pointerdown", () => this.keepOpen(), { signal: this.abort.signal });
    this.gallery.addEventListener("keydown", (event) => {
      if (event.key.toLowerCase() === "f") this.keepOpen();
    }, { capture: true, signal: this.abort.signal });
    this.body.className = "fx-port-preview-body";
    this.body.tabIndex = 0;
    this.sources.className = "fx-port-preview-sources";
    this.content.className = "fx-port-preview-content";
    footer.className = "fx-port-preview-footer";
    const close = document.createElement("button");
    close.type = "button";
    close.setAttribute("aria-label", "Close value preview");
    close.append(createElement(X, { width: 14, height: 14, "aria-hidden": "true" }));
    close.addEventListener("click", () => this.dismiss(true), { signal: this.abort.signal });
    labels.append(this.context, this.heading);
    header.append(labels, this.type, close);
    footer.append(this.hint);
    this.content.append(this.description, this.gallery, this.body, this.sources);
    this.element.append(header, this.content, footer);
    this.element.addEventListener("pointerenter", () => this.cancelClose(), { signal: this.abort.signal });
    this.element.addEventListener("pointerleave", () => this.scheduleClose(), { signal: this.abort.signal });
    this.element.addEventListener("focusin", () => this.cancelClose(), { signal: this.abort.signal });
    this.element.addEventListener("focusout", () => this.scheduleClose(), { signal: this.abort.signal });
    container.ownerDocument.addEventListener("pointerdown", (event) => {
      if (this.target && event.target instanceof Node && !this.element.contains(event.target) && !this.target.anchor.contains(event.target)) this.dismiss();
    }, { capture: true, signal: this.abort.signal });
    container.ownerDocument.addEventListener("keydown", (event) => {
      if (event.key !== "Escape" || !this.target || event.defaultPrevented) return;
      if (this.fullscreened) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      this.dismiss(true);
    }, { capture: true, signal: this.abort.signal });
    container.ownerDocument.addEventListener("fullscreenchange", () => {
      if (this.fullscreened) this.keepOpen();
      else if (this.target) this.position();
    }, { signal: this.abort.signal });
  }

  attach(anchor: SVGGElement, node: CanvasStep, port: CanvasPort, direction: InspectedPort["direction"], signal: AbortSignal) {
    const target: PortTarget = { anchor, node: node.id, title: node.title, port, name: port.name, direction, kind: port.kind };
    anchor.setAttribute("role", "button");
    anchor.setAttribute("tabindex", "0");
    anchor.setAttribute("aria-label", `Inspect ${direction} ${displayPortName(port.name)} of ${node.title}`);
    anchor.setAttribute("aria-controls", this.element.id);
    anchor.setAttribute("aria-expanded", "false");
    anchor.addEventListener("pointerenter", () => { if (this.canHover()) this.show(target); }, { signal });
    anchor.addEventListener("pointerleave", () => this.scheduleClose(), { signal });
    anchor.addEventListener("focusin", () => { if (!this.closing) this.show(target); }, { signal });
    anchor.addEventListener("focusout", () => this.scheduleClose(), { signal });
    const pin = (focusValue = false) => {
      if (this.target?.anchor === anchor && this.pinned) { this.dismiss(); return; }
      this.pinned = false;
      this.show(target);
      if (this.target?.anchor !== anchor) return;
      this.keepOpen();
      if (focusValue) (this.gallery.hidden ? this.body : this.imageList).focus({ preventScroll: true });
    };
    anchor.addEventListener("click", (event) => { event.preventDefault(); event.stopPropagation(); pin(); }, { signal });
    anchor.addEventListener("keydown", (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault(); event.stopPropagation(); pin(true);
    }, { signal });
  }

  private show(target: PortTarget) {
    this.cancelClose();
    if (this.pinned || this.covered(target.anchor)) return;
    if (this.target?.anchor === target.anchor) return;
    this.target?.anchor.setAttribute("aria-expanded", "false");
    this.target = target;
    target.anchor.setAttribute("aria-expanded", "true");
    const inspection = target.port.inspection;
    this.heading.textContent = displayPortName(target.name);
    this.context.textContent = `${target.direction === "input" ? "Input" : "Output"} · ${target.title}`;
    this.type.textContent = target.port.type;
    this.description.textContent = inspection?.description ?? "";
    this.description.hidden = !this.description.textContent;
    this.description.setAttribute("aria-label", `${displayPortName(target.name)} description`);
    this.body.textContent = inspection?.text ?? "Value not recorded";
    this.body.setAttribute("aria-label", `${displayPortName(target.name)} recorded value`);
    this.body.scrollTop = 0;
    this.content.scrollTop = 0;
    this.renderImages(inspection?.images ?? [], inspection?.image_count ?? 0);
    this.sources.textContent = inspection?.sources?.length ? `From ${inspection.sources.join(", ")}` : "";
    this.sources.hidden = !this.sources.textContent;
    this.hint.textContent = "Click the socket to keep open";
    this.element.setAttribute("aria-label", `${displayPortName(target.name)} value`);
    this.element.dataset.state = inspection?.state ?? "unavailable";
    this.element.classList.remove("is-pinned");
    this.element.hidden = false;
    this.position();
    this.onInspect(target);
  }

  private keepOpen() {
    if (!this.target) return;
    this.cancelClose();
    this.pinned = true;
    this.element.classList.add("is-pinned");
    this.hint.textContent = "Esc to close";
    if (!this.fullscreened) this.position();
  }

  private clearImages() {
    this.imageAbort?.abort();
    this.imageAbort = null;
    this.visual?.dispose();
    this.visual = null;
    this.imageList.replaceChildren();
    this.gallery.hidden = true;
    this.element.classList.remove("has-images");
  }

  private renderImages(images: readonly PortImage[], total: number) {
    this.clearImages();
    if (!images.length) return;
    this.imageAbort = new AbortController();
    this.gallery.hidden = false;
    this.gallery.classList.toggle("is-single", total === 1);
    this.element.classList.add("has-images");
    for (const [index, item] of images.entries()) {
      const figure = document.createElement("figure"), frame = document.createElement("div"), fallback = document.createElement("span");
      figure.className = "fx-port-image-tile";
      frame.className = "fx-port-image-frame";
      fallback.textContent = "Image unavailable";
      fallback.className = "fx-port-image-fallback";
      fallback.hidden = item.url !== null;
      frame.append(fallback);
      if (item.url !== null) {
        const image = document.createElement("img");
        image.alt = item.key === undefined ? item.name : `${item.key} · ${item.name}`;
        image.loading = index === 0 ? "eager" : "lazy";
        image.decoding = "async";
        image.addEventListener("error", () => { image.hidden = true; fallback.hidden = false; }, { signal: this.imageAbort.signal });
        image.src = item.url;
        frame.append(image);
      }
      figure.append(frame);
      if (total > 1) {
        const caption = document.createElement("figcaption");
        caption.textContent = item.key ?? item.name;
        caption.title = item.name;
        figure.append(caption);
      }
      this.imageList.append(figure);
    }
    this.imageCount.textContent = total > images.length ? `${images.length} of ${total} images` : `${total} image${total === 1 ? "" : "s"}`;
    this.imageCount.hidden = total <= 1;
    this.visual = new VisualPreviewController(this.gallery);
    this.imageList.scrollLeft = 0;
  }

  private position() {
    if (!this.target) return;
    // Use the same fixed row as pointer hit testing, so a flipped preview starts
    // beyond the labels instead of covering the sockets the pointer will visit next.
    const area = this.container.getBoundingClientRect(), anchor = (this.target.anchor.querySelector(".fx-canvas-port-hit") ?? this.target.anchor).getBoundingClientRect();
    const maximumWidth = Math.min(360, Math.max(0, area.width - 24));
    const spaces = { left: Math.max(0, anchor.left - area.left - 22), right: Math.max(0, area.right - anchor.right - 22) };
    const preferred = this.target.direction === "input" ? "left" : "right";
    const other = preferred === "left" ? "right" : "left";
    const side = spaces[preferred] >= maximumWidth ? preferred : spaces[other] >= maximumWidth ? other
      : spaces[preferred] >= spaces[other] ? preferred : other;
    const width = Math.min(maximumWidth, spaces[side] || maximumWidth);
    this.element.style.width = `${width}px`;
    this.element.style.maxHeight = `${Math.max(0, Math.min(420, area.height - 24))}px`;
    const right = anchor.right - area.left + 10, left = anchor.left - area.left - width - 10;
    const x = side === "left" ? left : right;
    this.element.style.left = `${Math.max(12, Math.min(x, area.width - width - 12))}px`;
    const height = this.element.getBoundingClientRect().height;
    this.element.style.top = `${Math.max(12, Math.min(anchor.top - area.top - 8, area.height - height - 12))}px`;
  }

  private cancelClose() { if (this.timer !== null) clearTimeout(this.timer); this.timer = null; }
  private scheduleClose() {
    this.cancelClose();
    if (this.pinned) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      if (!this.element.matches(":hover") && !this.element.contains(document.activeElement) && !this.target?.anchor.matches(":hover, :focus")) this.dismiss();
    }, 140);
  }

  dismiss(restoreFocus = false) {
    this.cancelClose();
    if (!this.target) return;
    const anchor = this.target?.anchor;
    const focusInside = this.element.contains(document.activeElement);
    anchor?.setAttribute("aria-expanded", "false");
    this.target = null;
    this.pinned = false;
    this.element.hidden = true;
    this.element.classList.remove("is-pinned");
    this.body.textContent = "";
    this.clearImages();
    this.description.textContent = "";
    this.description.hidden = true;
    this.sources.textContent = "";
    this.onInspect(null);
    if (restoreFocus && focusInside && anchor?.isConnected) {
      this.closing = true;
      anchor.focus({ preventScroll: true });
      this.closing = false;
    }
  }

  dispose() { this.dismiss(); this.abort.abort(); this.element.remove(); }
}
