/** Owns copying the setup command and its asynchronous clipboard feedback. */
export class GettingStartedController {
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly root: HTMLElement) {
    root.addEventListener("click", this.click);
  }

  dispose(): void {
    this.clearFeedback();
    this.root.removeEventListener("click", this.click);
  }

  private clearFeedback(): void {
    this.generation++;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    for (const button of this.root.querySelectorAll<HTMLElement>("[data-copy]")) {
      delete button.dataset.copied;
      button.setAttribute("aria-label", "Copy command");
    }
    const status = this.root.querySelector<HTMLElement>("[data-copy-status]");
    if (status) status.textContent = "";
  }

  private click = (event: MouseEvent): void => {
    if (!(event.target instanceof Element)) return;
    const copy = event.target.closest<HTMLButtonElement>("[data-copy]");
    if (copy && this.root.contains(copy)) void this.copy(copy);
  };

  private async copy(button: HTMLButtonElement): Promise<void> {
    const command = this.root.querySelector<HTMLElement>("[data-command]")?.textContent;
    if (!command) return;
    this.clearFeedback();
    const generation = this.generation;
    const status = this.root.querySelector<HTMLElement>("[data-copy-status]");
    try {
      await navigator.clipboard.writeText(command);
      if (generation !== this.generation) return;
      button.dataset.copied = "true";
      button.setAttribute("aria-label", "Command copied");
      if (status) status.textContent = "Copied to clipboard.";
      this.timer = setTimeout(() => this.clearFeedback(), 2000);
    } catch {
      if (generation !== this.generation) return;
      if (status) status.textContent = "Select the command to copy it manually.";
    }
  }
}
