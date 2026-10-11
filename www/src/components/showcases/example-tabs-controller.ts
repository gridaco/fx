/** Keyboard navigation and clipboard state, independent of React. */
export class ExampleTabsController {
  private disposed = false;
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;

  constructor(private readonly root: HTMLElement) {
    root.addEventListener("click", this.click);
    root.addEventListener("keydown", this.keydown);
  }

  dispose() {
    this.disposed = true;
    this.generation++;
    clearTimeout(this.timer);
    this.root.removeEventListener("click", this.click);
    this.root.removeEventListener("keydown", this.keydown);
  }

  private select(tab: HTMLButtonElement) {
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[role=tab]")) {
      const selected = button === tab;
      button.setAttribute("aria-selected", String(selected));
      button.tabIndex = selected ? 0 : -1;
    }
    for (const panel of this.root.querySelectorAll<HTMLElement>("[role=tabpanel]")) {
      const selected = panel.getAttribute("aria-labelledby") === tab.id;
      panel.style.visibility = selected ? "visible" : "hidden";
      panel.setAttribute("aria-hidden", String(!selected));
      panel.inert = !selected;
    }
    tab.focus();
  }

  private click = (event: MouseEvent) => {
    if (!(event.target instanceof Element)) return;
    const tab = event.target.closest<HTMLButtonElement>("[role=tab]");
    if (tab && this.root.contains(tab)) this.select(tab);
    const copy = event.target.closest<HTMLButtonElement>("[data-copy-example]");
    if (copy && this.root.contains(copy)) void this.copy(copy);
  };

  private keydown = (event: KeyboardEvent) => {
    if (!(event.target instanceof HTMLButtonElement) || event.target.getAttribute("role") !== "tab") return;
    const tabs = Array.from(this.root.querySelectorAll<HTMLButtonElement>("[role=tab]"));
    const index = tabs.indexOf(event.target);
    const next = event.key === "ArrowRight" ? (index + 1) % tabs.length
      : event.key === "ArrowLeft" ? (index + tabs.length - 1) % tabs.length
      : event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : null;
    if (next !== null) {
      event.preventDefault();
      this.select(tabs[next]);
    }
  };

  private async copy(button: HTMLButtonElement) {
    const panel = button.closest<HTMLElement>("[role=tabpanel]");
    const text = panel?.querySelector("[data-example-code]")?.textContent;
    if (!text || !panel || panel.inert) return;
    const references = Array.from(panel.querySelectorAll<HTMLAnchorElement>("[data-prompt-reference]")).map((reference) => ({ label: reference.dataset.referenceLabel ?? "Reference", href: reference.href }));
    const value = agentPromptClipboard(text, references, this.root.ownerDocument.baseURI);
    const generation = ++this.generation;
    const feedback = button.querySelector<HTMLElement>("[data-copy-label]");
    clearTimeout(this.timer);
    try {
      await navigator.clipboard.writeText(value);
      if (this.disposed || generation !== this.generation) return;
      if (feedback) feedback.textContent = "Copied";
      this.timer = setTimeout(() => { if (feedback) feedback.textContent = button.dataset.copyDefault ?? "Copy"; }, 1800);
    } catch {
      if (!this.disposed && generation === this.generation && feedback) feedback.textContent = "Copy failed. Try again.";
    }
  }
}
import { agentPromptClipboard } from "../../lib/agent-prompt";
