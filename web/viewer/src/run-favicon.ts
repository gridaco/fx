export type RunFaviconState = "idle" | "active" | "awaiting";

/** Chrome displays static favicon files, so pulse the active icon by swapping URLs. */
export class RunFavicon {
  private readonly idle: string;
  private readonly awaiting: string;
  private readonly frames: [string, string];
  private readonly motion = window.matchMedia("(prefers-reduced-motion: reduce)");
  private state: RunFaviconState = "idle";
  private timer?: number;

  constructor(private readonly icon: HTMLLinkElement) {
    this.idle = icon.href;
    this.awaiting = new URL("favicon-awaiting.svg", this.idle).href;
    this.frames = [
      new URL("favicon-active.svg", this.idle).href,
      new URL("favicon-active-dim.svg", this.idle).href,
    ];
    this.motion.addEventListener("change", this.update);
  }

  setState(state: RunFaviconState) {
    if (this.state === state) return;
    this.state = state;
    this.update();
  }

  private update = () => {
    window.clearInterval(this.timer);
    this.timer = undefined;
    this.icon.href = this.state === "awaiting" ? this.awaiting : this.state === "active" ? this.frames[0] : this.idle;
    if (this.state !== "active" || this.motion.matches) return;
    let frame = 0;
    this.timer = window.setInterval(() => {
      frame = 1 - frame;
      this.icon.href = this.frames[frame];
    }, 650);
  };

  dispose() {
    this.motion.removeEventListener("change", this.update);
    this.setState("idle");
  }
}
