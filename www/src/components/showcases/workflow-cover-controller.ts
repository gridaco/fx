type MotionPreference = Pick<MediaQueryList, "matches" | "addEventListener" | "removeEventListener">;
type ObserverFactory = (callback: IntersectionObserverCallback) => Pick<IntersectionObserver, "observe" | "disconnect">;

/** Covers load motion only while visible and when the visitor permits animation. */
export class WorkflowCoverController {
  private visible = false;
  private disposed = false;
  private attached = false;
  private readonly observer;
  private readonly preferenceChanged = () => this.sync();

  constructor(
    private readonly video: HTMLVideoElement,
    private readonly src: string,
    private readonly motion: MotionPreference = matchMedia("(prefers-reduced-motion: reduce)"),
    observe: ObserverFactory = callback => new IntersectionObserver(callback),
  ) {
    video.muted = true;
    this.observer = observe(entries => {
      this.visible = entries.some(entry => entry.target === video && entry.isIntersecting);
      this.sync();
    });
    motion.addEventListener("change", this.preferenceChanged);
    this.observer.observe(video);
  }

  private sync() {
    if (this.disposed) return;
    if (this.motion.matches) {
      this.video.pause();
      this.clearSource();
      return;
    }
    if (!this.visible) { this.video.pause(); return; }
    if (!this.attached) {
      this.video.src = this.src;
      this.attached = true;
    }
    // Autoplay may be refused by the browser; its poster remains a useful cover.
    void this.video.play().catch(() => {});
  }

  private clearSource() {
    if (!this.attached) return;
    this.video.removeAttribute("src");
    this.video.load();
    this.attached = false;
  }

  dispose() {
    this.disposed = true;
    this.observer.disconnect();
    this.motion.removeEventListener("change", this.preferenceChanged);
    this.video.pause();
    this.clearSource();
  }
}
