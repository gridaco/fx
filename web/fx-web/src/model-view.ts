import { createElement as icon, Pause, Play, type IconNode } from "lucide";
import type { Artifact } from "./index";
import { artifactDownloadName } from "./outputs";
import { ModelSourceController } from "./model";
import { loadModelRenderer, type ModelElement } from "./model-renderer";

function element<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag); node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** A recorded model's camera, playback, controls and renderer lifetime, independent of React. */
export class ModelPreviewController {
  private readonly abort = new AbortController();
  private readonly source = new ModelSourceController();
  private readonly viewport = element("div", "fx-model-viewport");
  private readonly status = element("p", "fx-model-status", "Loading 3D preview…");
  private readonly footer = element("div", "fx-model-footer");
  private readonly animationControls = element("div", "fx-model-controls");
  private readonly clips = element("select");
  private readonly play = this.button("Play", Play, () => this.toggle());
  private model?: ModelElement;
  private loadTimer?: ReturnType<typeof setTimeout>;
  private releaseSource?: () => void;
  private observer?: IntersectionObserver;
  private clipRevision = 0;

  constructor(private readonly root: HTMLElement, private readonly artifact: Artifact, private readonly options: { download?: boolean; defer?: boolean; onReady?: () => void; onError?: () => void } = {}) {
    root.classList.add("fx-model-preview"); root.dataset.phase = "loading";
    root.setAttribute("role", "group");
    root.setAttribute("aria-label", artifact.name || "3D preview");
    this.status.setAttribute("role", "status");
    this.viewport.append(this.status);
    this.clips.setAttribute("aria-label", "Animation");
    this.clips.addEventListener("change", () => { void this.selectClip(); }, { signal: this.abort.signal });
    this.animationControls.append(this.play, this.clips);
    this.animationControls.hidden = true;
    this.footer.append(this.animationControls);
    if (options.download !== false) {
      const download = element("a", "fx-model-download", artifact.name || "Download model");
      download.href = artifact.url!; download.download = artifactDownloadName(artifact);
      download.title = "Download original model";
      this.footer.append(download);
    } else this.footer.hidden = true;
    root.append(this.viewport, this.footer);
    // A collection can hold many models. Read and instantiate only those actually inspected.
    if (options.defer !== false && typeof IntersectionObserver !== "undefined") {
      this.observer = new IntersectionObserver((entries) => {
        if (entries.some((entry) => entry.isIntersecting)) { this.observer?.disconnect(); void this.load(); }
      });
      this.observer.observe(root);
    } else void this.load();
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) this.model?.pause();
      this.sync();
    }, { signal: this.abort.signal });
  }

  private button(label: string, glyph: IconNode, action: () => void): HTMLButtonElement {
    const button = element("button"); button.type = "button";
    this.buttonContent(button, label, glyph);
    button.addEventListener("click", action, { signal: this.abort.signal });
    return button;
  }

  private buttonContent(button: HTMLButtonElement, label: string, glyph: IconNode): void {
    button.setAttribute("aria-label", label); button.title = label;
    button.replaceChildren(icon(glyph, { width: 14, height: 14, "aria-hidden": "true" }));
  }

  private async load(): Promise<void> {
    if (this.releaseSource || this.abort.signal.aborted) return;
    this.loadTimer = setTimeout(() => this.fail("The 3D preview took too long to load. Download the original to inspect it elsewhere."), 30_000);
    this.releaseSource = this.source.subscribe(() => {
      const state = this.source.getSnapshot();
      if (state.phase === "error") this.fail(state.error!);
    });
    await this.source.load(this.artifact);
    if (this.abort.signal.aborted || this.source.getSnapshot().phase !== "ready") return;
    try {
      await loadModelRenderer();
      if (this.abort.signal.aborted || this.source.getSnapshot().phase !== "ready" || this.root.dataset.phase === "error") return;
      const model = document.createElement("model-viewer") as ModelElement;
      this.model = model;
      model.setAttribute("alt", this.artifact.name || "Recorded 3D model");
      model.title = "Drag to rotate · Scroll to zoom · Shift-drag to pan";
      model.setAttribute("camera-controls", ""); model.setAttribute("interaction-prompt", "none");
      model.setAttribute("environment-image", "neutral"); model.setAttribute("tone-mapping", "neutral");
      model.setAttribute("shadow-intensity", "0"); model.setAttribute("touch-action", "pan-y");
      model.setAttribute("animation-crossfade-duration", "0");
      model.setAttribute("loading", "eager"); model.setAttribute("reveal", "auto");
      model.addEventListener("load", () => this.loaded(), { signal: this.abort.signal });
      model.addEventListener("error", () => this.fail("The browser could not render this model. Its format, features, or graphics requirements may be unsupported."), { signal: this.abort.signal });
      for (const name of ["play", "pause", "finished"]) model.addEventListener(name, () => this.sync(), { signal: this.abort.signal });
      this.viewport.prepend(model);
      model.src = this.source.getSnapshot().url!;
    } catch {
      if (!this.abort.signal.aborted) this.fail("The 3D renderer is unavailable in this browser.");
    }
  }

  private loaded(): void {
    if (this.loadTimer) clearTimeout(this.loadTimer);
    if (!this.model || this.abort.signal.aborted || this.root.dataset.phase === "error") return;
    this.root.dataset.phase = "ready";
    this.status.hidden = true;
    const clips = this.model.availableAnimations;
    this.clips.replaceChildren(...clips.map((name, index) => {
      const duplicate = clips.indexOf(name) !== clips.lastIndexOf(name);
      const option = element("option", "", duplicate ? `${name || "Animation"} · clip ${index + 1}` : name || `Clip ${index + 1}`);
      option.value = name;
      if (duplicate || !name) {
        // The pinned renderer accepts an index when no clip matches animationName.
        // Avoid matching a numeric authored name before its numeric-index fallback.
        let selector = String(index);
        while (clips.includes(selector)) selector += " ";
        option.value = selector;
      }
      return option;
    }));
    this.clips.hidden = clips.length < 2;
    this.animationControls.hidden = clips.length === 0;
    if (this.options.download === false) this.footer.hidden = clips.length === 0;
    if (clips.length) void this.selectClip();
    this.model.cameraOrbit = "25deg 75deg auto";
    this.model.jumpCameraToGoal();
    this.options.onReady?.();
  }

  private async selectClip(): Promise<void> {
    const model = this.model, revision = ++this.clipRevision;
    if (!model) return;
    const wasPlaying = !model.paused;
    model.pause(); model.animationName = this.clips.value;
    // Wait for the renderer to commit the selected clip before resetting its time.
    // A stale selection must not reset a newer clip.
    await model.updateComplete;
    if (this.abort.signal.aborted || this.model !== model || revision !== this.clipRevision) return;
    model.currentTime = 0;
    if (wasPlaying) model.play();
    this.sync();
  }

  private toggle(): void {
    if (!this.model || !this.model.availableAnimations.length) return;
    if (this.model.paused) this.model.play(); else this.model.pause();
    this.sync();
  }

  private sync(): void {
    const model = this.model;
    if (!model || this.abort.signal.aborted || this.root.dataset.phase !== "ready") return;
    this.buttonContent(this.play, model.paused ? "Play" : "Pause", model.paused ? Play : Pause);
  }

  private fail(reason: string): void {
    if (this.loadTimer) clearTimeout(this.loadTimer);
    this.root.dataset.phase = "error";
    this.status.textContent = `3D preview unavailable. ${reason}`;
    this.status.hidden = false;
    this.animationControls.hidden = true;
    this.model?.pause(); this.model?.removeAttribute("src"); this.model?.remove(); this.model = undefined;
    this.source.dispose();
    this.options.onError?.();
  }

  dispose(): void {
    this.abort.abort(); this.observer?.disconnect(); this.releaseSource?.();
    if (this.loadTimer) clearTimeout(this.loadTimer);
    this.model?.pause(); this.model?.removeAttribute("src"); this.model?.remove();
    this.source.dispose();
    this.root.replaceChildren();
  }
}
