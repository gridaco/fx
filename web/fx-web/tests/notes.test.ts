import { expect, test } from "bun:test";
import { canvasGraph, isWorkflowNotes, isNoteTemplates, layoutGraph, noteMarkup, parseGraph, ViewerController, ViewerPresentation, type WorkflowNote, type LayoutReport } from "../src/index";
import { graph } from "./graph.test";
import { nestedPlan } from "./scopes.test";

function note(name: string, scope_id: string | null = null): WorkflowNote {
  return { id: `note:${scope_id ?? "root"}:${name}`, name, scope_id, about: null, format: "markdown", text: "## Read me\n\n**Literal** ${{ inputs.name }}" };
}

test("recorded notes are portable static presentation, distinct from executable members", () => {
  const input = graph(), before = canvasGraph(input);
  input.notes = [note("intro")];
  input.note_templates = [{ workflow: "workflows/example.yaml", container: "", name: "intro", about: null, format: "markdown", text: "Captured" }];
  const parsed = parseGraph(input), projected = canvasGraph(parsed);
  expect(projected.nodes).toEqual(before.nodes);
  expect(projected.edges).toEqual(before.edges);
  expect(projected.notes).toEqual(input.notes);
  expect(isNoteTemplates(input.note_templates)).toBeTrue();
  expect(isWorkflowNotes([note("intro"), note("intro")])).toBeFalse();
  expect(isWorkflowNotes([{ ...note("intro"), source: { path: "/private/note.md", digest: "a".repeat(64) } }])).toBeFalse();
  expect(isWorkflowNotes([{ ...note("intro"), format: "html" }])).toBeFalse();
  expect(isWorkflowNotes([{ ...note("intro"), id: "note:root:other" }])).toBeFalse();
  expect(isWorkflowNotes([{ ...note("intro"), source: { path: "notes/bad\u0085.md", digest: "a".repeat(64) } }])).toBeFalse();
  expect(() => parseGraph({ ...input, notes: [note("orphan", "scope:absent")] })).toThrow("does not match");
  expect(isWorkflowNotes([{ ...note("intro"), text: "leaked", withheld: true }])).toBeFalse();
  expect(isWorkflowNotes([{ ...note("intro"), text: null, withheld: true }])).toBeTrue();
});

test("note projection honors imported scope ownership and group frames", () => {
  const input = nestedPlan();
  input.notes = [note("root"), note("group", "scope:batch#1"), note("inside", "scope:batch.item#1"), note("nested", "scope:batch.item.child#1.1")];
  const root = canvasGraph(input);
  expect(root.notes?.map((item) => item.name)).toEqual(["root", "group"]);
  expect(root.notes?.find((item) => item.name === "group")?.frame_id).toBe("scope:batch#1");
  const inside = canvasGraph(input, "scope:batch.item#1");
  expect(inside.notes?.map((item) => item.name)).toEqual(["inside"]);
  expect(inside.notes?.[0].frame_id).toBeNull();
  expect(canvasGraph(input, "scope:batch.item.child#1.1").notes?.map((item) => item.name)).toEqual(["nested"]);
});

test("note geometry is stable as content changes and has no executable order membership", () => {
  const input = graph(); input.notes = [note("intro")];
  const projected = canvasGraph(input);
  const report: LayoutReport = { kind: "fx-layout-report-v1", file: null, revision: null, state: "none", cursor: null, diagnostics: [], cells: {}, order: [], notes: { [input.notes[0].id]: { column: 0, row: 2, source: "automatic" } } };
  const first = layoutGraph(projected, report);
  const second = layoutGraph({ ...projected, notes: [{ ...projected.notes![0], text: "A very long line".repeat(1000) }] }, report);
  expect(second.nodes).toEqual(first.nodes);
  expect(second.edges).toEqual(first.edges);
  const geometry = (value: typeof first) => value.notes?.map(({ x, y, width, height }) => ({ x, y, width, height }));
  expect(geometry(second)).toEqual(geometry(first));
  expect(first.decks.flatMap((deck) => deck.members)).not.toContain(input.notes[0].id);
});

test("CommonMark formats documentation but never evaluates FX expressions or raw HTML", () => {
  const html = noteMarkup("# Setup\n\n**Clear** reference\n\n`${{ inputs.prompt }}`\n\n<script>alert(1)</script>\n\n[bad](javascript:alert%281%29)");
  expect(html).toContain("<h1>Setup</h1>");
  expect(html).toContain("<strong>Clear</strong>");
  expect(html).toContain("${{ inputs.prompt }}");
  expect(html).not.toContain("<script>");
  expect(html).not.toContain('href="javascript:');
});

test("note focus uses recorded references and reveals nested notes without changing execution", async () => {
  const input = nestedPlan(); input.notes = [note("nested", "scope:batch.item.child#1.1")];
  const before = JSON.stringify(input);
  const controller = new ViewerController(async () => input);
  await controller.refresh();
  const presentation = new ViewerPresentation({ getSnapshot: controller.getSnapshot, restore: controller.restore.bind(controller) });
  presentation.attachCanvas(() => {});
  const result = presentation.present({ action: "reveal_note", note_id: input.notes[0].id });
  expect(result.scope_id).toBe("scope:batch.item.child#1.1");
  expect(result.selection).toEqual({ kind: "note", note_id: input.notes[0].id, scope_id: "scope:batch.item.child#1.1" });
  expect(JSON.stringify(result)).not.toContain("Literal");
  const state = controller.getSnapshot();
  expect(() => presentation.present({ action: "reveal_note", note_id: "note:missing" })).toThrow("does not exist");
  expect(controller.getSnapshot()).toBe(state);
  expect(JSON.stringify(input)).toBe(before);
});
