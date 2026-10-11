import { installOptions } from "./install-options";

/** Owns audience/installation choices and asynchronous clipboard feedback. */
export class GettingStartedController {
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly root: HTMLElement) {
    root.addEventListener("click", this.click);
    root.addEventListener("keydown", this.keydown);
    root.addEventListener("change", this.change);
  }

  dispose(): void {
    this.clearFeedback();
    this.root.removeEventListener("click", this.click);
    this.root.removeEventListener("keydown", this.keydown);
    this.root.removeEventListener("change", this.change);
  }

  private tabs(): HTMLButtonElement[] {
    return Array.from(this.root.querySelectorAll<HTMLButtonElement>("[role=tab]"));
  }

  private select(tab: HTMLButtonElement): void {
    this.clearFeedback();
    for (const candidate of this.tabs()) {
      const selected = candidate === tab;
      candidate.setAttribute("aria-selected", String(selected));
      candidate.tabIndex = selected ? 0 : -1;
    }
    for (const panel of this.root.querySelectorAll<HTMLElement>("[role=tabpanel]")) {
      panel.hidden = panel.dataset.panel !== tab.dataset.audience;
    }
    tab.focus();
  }

  private clearFeedback(): void {
    this.generation++;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    for (const button of this.root.querySelectorAll<HTMLElement>("[data-copy]")) {
      delete button.dataset.copied;
      button.setAttribute("aria-label", "Copy setup command");
    }
    const status = this.root.querySelector<HTMLElement>("[data-copy-status]");
    if (status) status.textContent = "";
  }

  private click = (event: MouseEvent): void => {
    if (!(event.target instanceof Element)) return;
    const tab = event.target.closest<HTMLButtonElement>("[role=tab]");
    if (tab && this.root.contains(tab)) this.select(tab);
    const copy = event.target.closest<HTMLButtonElement>("[data-copy]");
    if (copy && this.root.contains(copy)) void this.copy(copy);
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

  private change = (event: Event): void => {
    if (!(event.target instanceof HTMLSelectElement) || !event.target.matches("[data-install-option]")) return;
    const value = event.target.value;
    const option = installOptions.find(({ id }) => id === value);
    const panel = event.target.closest<HTMLElement>("[role=tabpanel]");
    if (!option || !panel || panel.hidden) return;
    this.clearFeedback();
    const command = panel.querySelector<HTMLElement>("[data-command]");
    if (command) command.textContent = option.command;
  };

  private async copy(button: HTMLButtonElement): Promise<void> {
    const panel = button.closest<HTMLElement>("[role=tabpanel]");
    const command = panel?.querySelector<HTMLElement>("[data-command]")?.textContent;
    if (!command || panel?.hidden) return;
    this.clearFeedback();
    const generation = this.generation;
    const status = this.root.querySelector<HTMLElement>("[data-copy-status]");
    try {
      await navigator.clipboard.writeText(command);
      if (generation !== this.generation) return;
      button.dataset.copied = "true";
      button.setAttribute("aria-label", "Setup command copied");
      if (status) status.textContent = "Copied to clipboard.";
      this.timer = setTimeout(() => this.clearFeedback(), 2000);
    } catch {
      if (generation !== this.generation) return;
      if (status) status.textContent = "Select the command to copy it manually.";
    }
  }
}
