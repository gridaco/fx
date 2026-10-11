import { animate } from "motion/mini";

/** A quiet fade of the whole composition; content is visible without JavaScript. */
export class LandingMotionController {
  private readonly reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  private animation: ReturnType<typeof animate> | null = null;

  constructor(private readonly root: HTMLElement) {
    this.reducedMotion.addEventListener("change", this.preferenceChanged);
    if (this.reducedMotion.matches || typeof root.animate !== "function") return;

    this.animation = animate(root, { opacity: [0.88, 1] }, {
      duration: 0.24,
      ease: [0.2, 0, 0, 1],
    });
  }

  dispose(): void {
    this.reducedMotion.removeEventListener("change", this.preferenceChanged);
    this.reset();
  }

  private preferenceChanged = (): void => {
    if (this.reducedMotion.matches) this.reset();
  };

  private reset(): void {
    this.animation?.cancel();
    this.animation = null;
    this.root.style.removeProperty("opacity");
  }
}
