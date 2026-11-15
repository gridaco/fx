import { expect, test } from "bun:test";
import { canvasGraph, isWorkflowNotes, isNoteTemplates, layoutGraph, noteMarkup, parseGraph, ViewerController, ViewerPresentation, type CanvasGraph, type WorkflowNote, type LayoutReport } from "../src/index";
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

test("content-sized notes reserve their row heights without moving executable cells", () => {
  const input = graph(); input.notes = [note("short"), note("ordinary"), note("long")];
  const projected = canvasGraph(input);
  const report: LayoutReport = { kind: "fx-layout-report-v1", file: null, revision: null, state: "none", cursor: null, diagnostics: [], cells: {}, order: [],
    notes: Object.fromEntries(input.notes.map((item, index) => [item.id, { column: 0, row: index + 1, source: "automatic" as const }])) };
  const original = JSON.stringify(report);
  const compact = new Map(input.notes.map((item) => [item.id, { width: 360, height: 96 }]));
  const measured = new Map(input.notes.map((item, index) => [item.id, { width: 360, height: [96, 360, 720][index] }]));
  const first = layoutGraph(projected, report, compact);
  const second = layoutGraph(projected, report, measured);
  expect(second.nodes).toEqual(first.nodes);
  expect(second.edges).toEqual(first.edges);
  expect(second.notes!.map((item) => item.height)).toEqual([96, 360, 720]);
  for (const [index, current] of second.notes!.entries()) {
    const next = second.notes![index + 1];
    if (next) expect(next.y).toBeGreaterThan(current.y + current.height);
    expect(second.height).toBeGreaterThan(current.y + current.height);
  }
  expect(second.notes![2].y).toBeGreaterThan(first.notes![2].y);
  expect(second.decks.flatMap((deck) => deck.members).some((id) => id.startsWith("note:"))).toBeFalse();
  expect(JSON.stringify(report)).toBe(original);
});

test("repeated group frames share measured tracks and keep their notes inside the frame", () => {
  const first = { ...note("guide", "group#1"), frame_id: "group#1" };
  const second = { ...note("guide", "group#2"), frame_id: "group#2" };
  const after = note("after");
  const graph: CanvasGraph = { nodes: [], edges: [], notes: [first, second, after], frames: [
    { id: "group#1", title: "First", address: "group", nodes: [] },
    { id: "group#2", title: "Second", address: "group", nodes: [] },
  ] };
  const report: LayoutReport = { kind: "fx-layout-report-v1", file: null, revision: null, state: "none", cursor: null, diagnostics: [],
    cells: { group: { column: 0, row: 0, source: "automatic" } }, order: [], notes: {
      [first.id]: { column: 0, row: 0, source: "automatic" },
      [second.id]: { column: 0, row: 0, source: "automatic" },
      [after.id]: { column: 0, row: 1, source: "automatic" },
    } };
  const compact = new Map(graph.notes!.map((item) => [item.id, { width: 360, height: 96 }]));
  const sizes = new Map(compact); sizes.set(second.id, { width: 360, height: 720 });
  const before = layoutGraph(graph, report, compact), measured = layoutGraph(graph, report, sizes);
  expect(measured.frames[0].height).toBe(measured.frames[1].height);
  expect(measured.frames[0].height).toBeGreaterThan(before.frames[0].height);
  for (const frame of measured.frames) {
    const inside = measured.notes!.find((item) => item.frame_id === frame.id)!;
    expect(inside.y).toBeGreaterThan(frame.y);
    expect(inside.y + inside.height).toBeLessThan(frame.y + frame.height);
  }
  const following = measured.notes!.find((item) => item.id === after.id)!;
  expect(following.y).toBeGreaterThan(Math.max(...measured.frames.map((frame) => frame.y + frame.height)));
  expect(following.y).toBeGreaterThan(before.notes!.find((item) => item.id === after.id)!.y);
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
