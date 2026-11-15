import { micromark } from "micromark";

/** A captured documentation occurrence, independent of executable instances. */
export interface WorkflowNote {
  id: string;
  name: string;
  scope_id: string | null;
  about: string | null;
  format: "markdown" | "text";
  text: string | null;
  source?: { path: string; digest: string };
  diagnostic?: string;
  withheld?: true;
}
/** Declarations captured before a dynamic scope exists; never rendered directly. */
export interface NoteTemplate extends Omit<WorkflowNote, "id" | "scope_id" | "withheld"> { workflow: string; container: string }
export interface CanvasNote extends WorkflowNote { frame_id?: string | null }

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const fields = (value: Record<string, unknown>, required: string[], optional: string[] = []) => required.every((key) => key in value) && Object.keys(value).every((key) => required.includes(key) || optional.includes(key));
const name = (value: unknown): value is string => typeof value === "string" && /^[a-z][a-z0-9_]*$/.test(value);
const label = (value: unknown): value is string => typeof value === "string" && value.length > 0 && !/[\\:\u0000-\u001f\u007f-\u009f]/.test(value) && value.split("/").every((part) => part !== "" && part !== "." && part !== "..");
function content(value: Record<string, unknown>) {
  return name(value.name) && (value.about === null || typeof value.about === "string")
    && (value.format === "markdown" || value.format === "text") && (value.text === null || typeof value.text === "string")
    && (value.diagnostic === undefined || typeof value.diagnostic === "string")
    && (value.source === undefined || record(value.source) && fields(value.source, ["path", "digest"]) && label(value.source.path) && typeof value.source.digest === "string" && /^[a-f0-9]{64}$/.test(value.source.digest));
}
export function isWorkflowNote(value: unknown): value is WorkflowNote {
  return record(value) && fields(value, ["id", "name", "scope_id", "about", "format", "text"], ["source", "diagnostic", "withheld"])
    && value.id === `note:${value.scope_id ?? "root"}:${value.name}`
    && (value.scope_id === null || typeof value.scope_id === "string" && value.scope_id.length > 0)
    && (value.withheld === undefined || value.withheld === true && value.text === null && value.source === undefined)
    && content(value);
}
export function isWorkflowNotes(value: unknown): value is WorkflowNote[] {
  return Array.isArray(value) && value.every(isWorkflowNote) && new Set(value.map((note) => note.id)).size === value.length;
}
export function isNoteTemplates(value: unknown): value is NoteTemplate[] {
  return Array.isArray(value) && value.every((item) => record(item)
    && fields(item, ["workflow", "container", "name", "about", "format", "text"], ["source", "diagnostic"])
    && typeof item.workflow === "string" && typeof item.container === "string" && content(item));
}

/** CommonMark only: raw HTML and dangerous protocols stay escaped/disabled. */
export function noteMarkup(text: string): string {
  return micromark(text, { allowDangerousHtml: false, allowDangerousProtocol: false });
}
/** Render captured documentation without fetching embedded resources or evaluating expressions. */
export function renderNoteContent(note: Pick<WorkflowNote, "format" | "text" | "withheld" | "diagnostic">, doc: Document = document, excerpt = false): HTMLElement {
  const body = doc.createElement("div");
  body.className = `fx-note-content${note.format === "text" ? " fx-note-plain" : ""}`;
  const text = excerpt ? note.text?.slice(0, 4096) ?? null : note.text;
  if (text === null) {
    body.classList.add("fx-note-unavailable");
    body.textContent = note.withheld ? "Withheld by the author" : "Note unavailable";
  } else if (note.format === "text") body.textContent = text;
  else {
    // A template is inert: resource-bearing elements are removed before insertion in the page.
    const template = doc.createElement("template");
    template.innerHTML = noteMarkup(text);
    for (const image of template.content.querySelectorAll("img")) image.replaceWith(doc.createTextNode(image.getAttribute("alt") ?? ""));
    for (const link of template.content.querySelectorAll("a")) {
      const href = link.getAttribute("href") ?? "";
      let safe = false;
      try { const url = new URL(href); safe = ["http:", "https:"].includes(url.protocol) && !url.username && !url.password; } catch { /* Relative links have no portable file target yet. */ }
      if (!safe) link.replaceWith(...link.childNodes);
      else { link.target = "_blank"; link.rel = "noopener noreferrer"; }
    }
    body.append(template.content);
  }
  if (note.diagnostic) {
    const diagnostic = doc.createElement("p");
    diagnostic.className = "fx-note-diagnostic";
    diagnostic.textContent = note.diagnostic;
    body.append(diagnostic);
  }
  return body;
}
