import { NOTE_MAX_HEIGHT, NOTE_MIN_HEIGHT, NOTE_WIDTH } from "./layout";
import { renderNoteContent, type CanvasNote } from "./notes";

/** A long source remains available in full inspection, without parsing it all for a canvas card. */
const CANVAS_TEXT_LIMIT = 32 * 1024;
const CONTENT_TOP = 40, BOTTOM_PAD = 16, COLLAPSED_BOTTOM = 36;

export interface NoteCardContent {
  body: HTMLElement;
  collapsed: boolean;
  contentHeight: number;
}
interface PreparedNote extends NoteCardContent {
  format: CanvasNote["format"];
  text: CanvasNote["text"];
  diagnostic: CanvasNote["diagnostic"];
  withheld: CanvasNote["withheld"];
  height: number;
}

/**
 * Sizes the same sanitized HTML the card displays before tracks, frames and camera fit are
 * calculated. There is no visible provisional card and no measurement feedback loop.
 */
export class NoteCards {
  private readonly host: HTMLDivElement;
  private readonly prepared = new Map<string, PreparedNote>();

  constructor(private readonly container: HTMLElement) {
    this.host = container.ownerDocument.createElement("div");
    this.host.setAttribute("aria-hidden", "true");
    Object.assign(this.host.style, {
      position: "absolute", left: "0", top: "0", width: `${NOTE_WIDTH - 32}px`,
      visibility: "hidden", pointerEvents: "none", contain: "layout style",
    });
  }

  prepare(notes: readonly CanvasNote[]): ReadonlyMap<string, { width: number; height: number }> {
    const present = new Set(notes.map((note) => note.id));
    for (const id of this.prepared.keys()) if (!present.has(id)) this.prepared.delete(id);
    const changed: { note: CanvasNote; body: HTMLElement; truncated: boolean }[] = [];
    for (const note of notes) {
      const previous = this.prepared.get(note.id);
      if (previous && previous.format === note.format && previous.text === note.text
        && previous.diagnostic === note.diagnostic && previous.withheld === note.withheld) continue;
      // Keep a surrogate pair together at the preview boundary.
      let end = CANVAS_TEXT_LIMIT;
      if (note.text && /[\uD800-\uDBFF]/.test(note.text.charAt(end - 1))) end--;
      const truncated = (note.text?.length ?? 0) > end;
      const body = renderNoteContent(truncated ? { ...note, text: note.text!.slice(0, end) } : note, this.container.ownerDocument);
      body.classList.add("fx-note-card-content");
      Object.assign(body.style, { display: "flow-root", width: `${NOTE_WIDTH - 32}px` });
      // A card has one activation target. Links become active in full inspection.
      for (const link of body.querySelectorAll("a")) link.removeAttribute("href");
      changed.push({ note, body, truncated });
    }
    if (changed.length) {
      this.host.replaceChildren(...changed.map((item) => item.body));
      this.container.append(this.host);
      // An SDK host can prepare its viewer before mounting it, or in a hidden tab.
      // The shared content styles still apply when measuring under the document body.
      if (!this.host.getClientRects().length) (this.container.ownerDocument.body ?? this.container.ownerDocument.documentElement).append(this.host);
      try {
        for (const { note, body, truncated } of changed) {
          // CSS layout units stay correct when an embedding host scales its whole viewer.
          // One pixel covers fractional line boxes rounded by the integer DOM metrics.
          const measured = Math.max(body.scrollHeight, body.offsetHeight) + 1;
          const collapsed = truncated || measured + CONTENT_TOP + BOTTOM_PAD > NOTE_MAX_HEIGHT;
          const bottom = collapsed ? COLLAPSED_BOTTOM : BOTTOM_PAD;
          const height = Math.max(NOTE_MIN_HEIGHT, Math.min(NOTE_MAX_HEIGHT, measured + CONTENT_TOP + bottom));
          this.prepared.set(note.id, { body, collapsed, contentHeight: height - CONTENT_TOP - bottom, height,
            format: note.format, text: note.text, diagnostic: note.diagnostic, withheld: note.withheld });
        }
      } finally {
        this.host.replaceChildren();
        this.host.remove();
      }
    }
    return new Map(notes.map((note) => [note.id, { width: NOTE_WIDTH, height: this.prepared.get(note.id)!.height }]));
  }

  content(id: string): NoteCardContent | undefined {
    const entry = this.prepared.get(id);
    return entry && { body: entry.body.cloneNode(true) as HTMLElement, collapsed: entry.collapsed, contentHeight: entry.contentHeight };
  }

  dispose() { this.prepared.clear(); this.host.remove(); }
}
