/** Owns the example tabs and local media playback. No workflow runs in the browser. */
export class LandingShowcaseController {
  private readonly reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  private readonly observer: IntersectionObserver;
  private active = "worlds";
  private playing = !this.reducedMotion.matches;
  private visible = true;
  private disposed = false;
  private playGeneration = 0;

  constructor(private readonly root: HTMLElement) {
    root.addEventListener("click", this.click);
    root.addEventListener("keydown", this.keydown);
    document.addEventListener("visibilitychange", this.syncPlayback);
    this.reducedMotion.addEventListener("change", this.preferenceChanged);
    this.observer = new IntersectionObserver(([entry]) => {
      this.visible = entry.isIntersecting;
      this.syncPlayback();
    });
    this.observer.observe(root);
    this.syncPlayback();
  }

  dispose(): void {
    this.disposed = true;
    this.playGeneration++;
    this.observer.disconnect();
    this.root.removeEventListener("click", this.click);
    this.root.removeEventListener("keydown", this.keydown);
    document.removeEventListener("visibilitychange", this.syncPlayback);
    this.reducedMotion.removeEventListener("change", this.preferenceChanged);
    this.root.dataset.playing = "false";
    this.root.querySelector("video")?.pause();
  }

  private tabs(): HTMLButtonElement[] {
    return Array.from(this.root.querySelectorAll<HTMLButtonElement>("[role=tab]"));
  }

  private select(tab: HTMLButtonElement): void {
    this.active = tab.dataset.showcaseTab!;
    for (const candidate of this.tabs()) {
      const selected = candidate === tab;
      candidate.setAttribute("aria-selected", String(selected));
      candidate.tabIndex = selected ? 0 : -1;
    }
    for (const panel of this.root.querySelectorAll<HTMLElement>("[role=tabpanel]")) {
      panel.hidden = panel.dataset.showcasePanel !== this.active;
    }
    this.root.dataset.example = this.active;
    this.syncPlayback();
    tab.focus();
  }

  private click = (event: MouseEvent): void => {
    if (!(event.target instanceof Element)) return;
    const tab = event.target.closest<HTMLButtonElement>("[data-showcase-tab]");
    if (tab) this.select(tab);
    if (event.target.closest("[data-showcase-playback]")) {
      this.playing = !this.playing;
      this.syncPlayback();
    }
  };

  private keydown = (event: KeyboardEvent): void => {
    if (!(event.target instanceof HTMLButtonElement) || event.target.getAttribute("role") !== "tab") return;
    const tabs = this.tabs();
    const index = tabs.indexOf(event.target);
    const next = event.key === "ArrowRight" ? (index + 1) % tabs.length
      : event.key === "ArrowLeft" ? (index + tabs.length - 1) % tabs.length
      : event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : null;
    if (next === null) return;
    event.preventDefault();
    this.select(tabs[next]);
  };

  private preferenceChanged = (): void => {
    this.playing = !this.reducedMotion.matches;
    this.syncPlayback();
  };

  private syncPlayback = (): void => {
    if (this.disposed) return;
    const generation = ++this.playGeneration;
    const shouldPlay = this.playing && this.visible && !document.hidden;
    this.root.dataset.playing = String(shouldPlay);
    this.root.dataset.paused = String(!this.playing);
    const control = this.root.querySelector<HTMLButtonElement>("[data-showcase-playback]");
    if (control) {
      control.disabled = this.active === "materials";
      control.setAttribute("aria-label", this.playing ? "Pause preview" : "Play preview");
    }
    const video = this.root.querySelector<HTMLVideoElement>("video");
    if (!video) return;
    if (this.active !== "campaigns" || !shouldPlay) {
      video.pause();
      return;
    }
    // The film is fetched only when its example is selected and playback is allowed.
    if (!video.hasAttribute("src")) video.src = video.dataset.src!;
    void video.play().catch(() => {
      if (generation !== this.playGeneration || this.disposed) return;
      this.playing = false;
      this.syncPlayback();
    });
  };
}
