/** Owns local preview playback. No workflow runs in the browser. */
export class LandingShowcaseController {
  private readonly reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  private readonly observer: IntersectionObserver;
  private playing = !this.reducedMotion.matches;
  private visible = true;
  private disposed = false;

  constructor(private readonly root: HTMLElement) {
    root.addEventListener("click", this.click);
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
    this.observer.disconnect();
    this.root.removeEventListener("click", this.click);
    document.removeEventListener("visibilitychange", this.syncPlayback);
    this.reducedMotion.removeEventListener("change", this.preferenceChanged);
    this.root.dataset.playing = "false";
  }

  private click = (event: MouseEvent): void => {
    if (!(event.target instanceof Element) || !event.target.closest("[data-showcase-playback]")) return;
    this.playing = !this.playing;
    this.syncPlayback();
  };

  private preferenceChanged = (): void => {
    this.playing = !this.reducedMotion.matches;
    this.syncPlayback();
  };

  private syncPlayback = (): void => {
    if (this.disposed) return;
    this.root.dataset.playing = String(this.playing && this.visible && !document.hidden);
    this.root.dataset.paused = String(!this.playing);
    this.root.querySelector<HTMLButtonElement>("[data-showcase-playback]")?.setAttribute("aria-label", this.playing ? "Pause preview" : "Play preview");
  };
}
