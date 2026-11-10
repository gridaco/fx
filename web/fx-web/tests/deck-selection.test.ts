import { describe, expect, test } from "bun:test";
import { canvasGraph, layoutGraph, type CanvasGraph, type CanvasStep, type RunNode, type ViewerRun } from "../src/index";
import { DeckSelection } from "../src/deck-selection";

function runNode(step: string, take: number, state = "succeeded", patch: Partial<RunNode> = {}): RunNode {
  return { id: `${step}#${take}`, path: step, step, take: [take], key: null, title: step, uses: `./nodes.py#${step}`,
    state, reads: [], with: {}, outputs: {}, cache: null, error: null, duration_ms: state === "skipped" ? null : 0, ...patch };
}

function takes(picked = 2): ViewerRun {
  return { kind: "fx-viewer-run-v1", workflow: { id: "acme", title: "Candidates" }, run_name: "saved", state: "succeeded", stand_in: false,
    charged_usd: 0, estimate: null, inputs: {}, outputs: {}, artifacts: [], warnings: [], nodes: [
      ...[1, 2, 3].map((take) => runNode("draw", take, take === 3 ? "skipped" : "succeeded")),
      ...[1, 2, 3].map((take) => runNode("review", take, take === 3 ? "skipped" : "succeeded", { reads: [`draw#${take}`], judges: `draw#${take}` })),
      runNode("finish", 1, "succeeded", { reads: [`draw#${picked}`, `review#${picked}`] }),
    ] };
}

function selection(view: ViewerRun) {
  const layout = layoutGraph(canvasGraph(view));
  const model = new DeckSelection(layout);
  const deck = (address: string) => layout.decks.findIndex((deck) => deck.address === address);
  const front = (address: string, selected: string | null = null) => {
    const index = deck(address);
    return layout.decks[index].members[model.front(index, selected)];
  };
  return { layout, model, deck, front };
}

describe("collapsed representative from recorded evidence", () => {
  test("unused future take never becomes consumed through its planned review wiring", () => {
    const view = takes(), before = JSON.stringify(view);
    const { front, model, deck } = selection(view);
    expect(front("draw")).toBe("instance:draw#2");
    expect(front("review")).toBe("instance:review#2");
    expect(model.members[deck("draw")].map((member) => member.bound === true)).toEqual([false, true, false]);
    expect(JSON.stringify(view)).toBe(before);
  });

  test("an earlier consumed take beats a later completed and judged alternative", () => {
    const view = takes(1);
    const { front } = selection(view);
    expect(front("draw")).toBe("instance:draw#1");
    expect(front("review")).toBe("instance:review#1");
  });

  test("explicit inspection of an unused take or its review wins and restores independently", () => {
    const { front, model, deck } = selection(takes());
    for (const selected of ["instance:draw#3", "instance:review#3"]) {
      expect(front("draw", selected)).toBe("instance:draw#3");
      expect(front("review", selected)).toBe("instance:review#3");
    }
    expect(new Set(model.affected("instance:review#3"))).toEqual(new Set([deck("draw"), deck("review")]));
    expect(front("draw")).toBe("instance:draw#2");
    expect(front("review")).toBe("instance:review#2");
  });

  test("corresponding hover members include both partners and leave defaults untouched", () => {
    const { front, model, deck } = selection(takes());
    expect(model.partners(deck("draw"), 2)).toEqual([{ deck: deck("draw"), card: 2 }, { deck: deck("review"), card: 2 }]);
    expect(model.partners(deck("review"), 0)).toEqual([{ deck: deck("draw"), card: 0 }, { deck: deck("review"), card: 0 }]);
    expect(front("draw")).toBe("instance:draw#2");
  });

  test("attention priorities are shared by partners and preserve their existing order", () => {
    const view = takes(1);
    view.nodes.find((node) => node.id === "draw#2")!.state = "running";
    view.nodes.find((node) => node.id === "review#3")!.state = "failed";
    expect(selection(view).front("draw")).toBe("instance:draw#3");
    view.nodes.find((node) => node.id === "review#3")!.state = "skipped";
    expect(selection(view).front("review")).toBe("instance:review#2");
    view.nodes.find((node) => node.id === "draw#2")!.state = "waiting";
    expect(selection(view).front("draw")).toBe("instance:draw#2");
  });

  test("ambiguous legacy execution evidence falls back without inventing consumption", () => {
    const view = takes(1);
    view.nodes.forEach((node) => { node.duration_ms = null; });
    const { front, model, deck } = selection(view);
    expect(model.members[deck("draw")].some((member) => member.bound)).toBeFalse();
    expect(front("draw")).toBe("instance:draw#2");
  });

  test("all skipped remains deterministic; a skipped operation may have execution evidence", () => {
    const view = takes();
    view.nodes.forEach((node) => { node.state = "skipped"; node.duration_ms = null; });
    expect(selection(view).front("draw")).toBe("instance:draw#1");
    view.nodes.find((node) => node.id === "draw#2")!.duration_ms = 0;
    expect(selection(view).front("draw")).toBe("instance:draw#2");
    expect(selection(view).front("review")).toBe("instance:review#2");
  });

  test("canonical member order, geometry and the members' own previews do not change", () => {
    const graph = canvasGraph(takes(1));
    const draw = graph.nodes.find((node) => node.source_id === "draw#2")!;
    draw.preview = { kind: "video", digest: "a".repeat(64), label: "Unavailable original", url: null, count: 1 };
    const layout = layoutGraph(graph), before = JSON.stringify(layout), model = new DeckSelection(layout);
    const drawDeck = layout.decks.findIndex((deck) => deck.address === "draw");
    expect(layout.decks[drawDeck].members[model.front(drawDeck, null)]).toBe("instance:draw#1");
    for (let deck = 0; deck < layout.decks.length; deck++) {
      model.front(deck, null);
      for (const member of layout.decks[deck].members) model.front(deck, member);
      expect(layout.decks[deck].members.map((id) => layout.nodes.find((node) => node.id === id)!.take)).toEqual([[1], [2], [3]]);
    }
    expect(JSON.stringify(layout)).toBe(before);
    expect(layout.nodes.find((node) => node.source_id === "draw#3")!.preview).toBeUndefined();
  });
});

function step(id: string, address: string, take: number[], key: string | null = null): CanvasStep {
  return { id, source_id: id, address, take, key, title: address, subtitle: "./nodes.py#render", state: "succeeded", pending: false, executed: true };
}

describe("conservative deck correspondence", () => {
  test("item decks retain their fallback instead of applying the take-consumption priority", () => {
    const graph: CanvasGraph = { nodes: [step("a1", "a", [1], "first"), step("a2", "a", [1], "second"), step("finish", "finish", [1])],
      edges: [{ id: "read", source: "a1", target: "finish", kinds: ["reads"], consumed: true }] };
    expect(new DeckSelection(layoutGraph(graph)).front(0, null)).toBe(1);
  });

  test("an unfamiliar status alone does not invent execution evidence", () => {
    const graph: CanvasGraph = { nodes: [{ ...step("a1", "a", [1]), state: "pending" }, { ...step("a2", "a", [2]), state: "future_state" }], edges: [] };
    graph.nodes.forEach((node) => { delete node.executed; });
    expect(new DeckSelection(layoutGraph(graph)).front(0, null)).toBe(0);
    graph.nodes[1].executed = true;
    expect(new DeckSelection(layoutGraph(graph)).front(0, null)).toBe(1);
  });

  test("matching take numbers without an explicit judge relationship leave decks independent", () => {
    const graph: CanvasGraph = { nodes: [step("a1", "a", [1]), step("a2", "a", [2]), step("b1", "b", [1]), step("b2", "b", [2])], edges: [] };
    const layout = layoutGraph(graph), model = new DeckSelection(layout);
    expect(model.partners(0, 0)).toEqual([{ deck: 0, card: 0 }]);
    expect(model.front(1, "a1")).toBe(1);
  });

  test("partial, many-to-one and mismatched full take paths are not paired", () => {
    for (const mode of ["partial", "ambiguous", "path", "key"]) {
      const graph: CanvasGraph = { nodes: [step("a1", "a", [1, 1]), step("a2", "a", [1, 2]), step("b1", "b", [1, 1]), step("b2", "b", mode === "path" ? [2, 2] : [1, 2], mode === "key" ? "different" : null)],
        edges: [{ id: "j1", source: "a1", target: "b1", kind: "control", kinds: ["judges"] },
          ...(mode === "partial" ? [] : [{ id: "j2", source: "a2", target: mode === "ambiguous" ? "b1" : "b2", kind: "control" as const, kinds: ["judges"] }])] };
      expect(new DeckSelection(layoutGraph(graph)).partners(0, 0)).toEqual([{ deck: 0, card: 0 }]);
    }
  });

  test("multiple judges share a representative with their subject", () => {
    const view = takes(1);
    view.nodes.push(...[1, 2, 3].map((take) => runNode("second_review", take, take === 3 ? "skipped" : "succeeded", { judges: `draw#${take}`, reads: [`draw#${take}`] })));
    const { front, model, deck } = selection(view);
    expect(front("second_review")).toBe("instance:second_review#1");
    expect(model.partners(deck("second_review"), 2)).toHaveLength(3);
  });

  test("partners match recorded identity even when their canonical orders differ", () => {
    const graph: CanvasGraph = { nodes: [step("a1", "a", [1]), step("a2", "a", [2]), step("b1", "b", [2]), step("b2", "b", [1])],
      edges: [{ id: "j1", source: "a1", target: "b2", kinds: ["judges"] }, { id: "j2", source: "a2", target: "b1", kinds: ["judges"] }] };
    const layout = layoutGraph(graph), before = JSON.stringify(layout), model = new DeckSelection(layout);
    expect(model.front(0, "b2")).toBe(0);
    expect(model.front(1, "a1")).toBe(1);
    expect(model.partners(0, 1)).toEqual([{ deck: 0, card: 1 }, { deck: 1, card: 0 }]);
    expect(JSON.stringify(layout)).toBe(before);
  });

  test("matching judge decks in different frames remain independent", () => {
    const graph: CanvasGraph = { nodes: [step("a1", "a", [1]), step("a2", "a", [2]), step("b1", "b", [1]), step("b2", "b", [2])],
      frames: [{ id: "left", title: "left", nodes: ["a1", "a2"] }, { id: "right", title: "right", nodes: ["b1", "b2"] }],
      edges: [{ id: "j1", source: "a1", target: "b1", kinds: ["judges"] }, { id: "j2", source: "a2", target: "b2", kinds: ["judges"] }] };
    const layout = layoutGraph(graph), model = new DeckSelection(layout), at = layout.decks.findIndex((deck) => deck.address === "a");
    expect(model.partners(at, 0)).toEqual([{ deck: at, card: 0 }]);
  });

  test("a frame's evidence and explicit descendant inspection stay within its membership", () => {
    const graph: CanvasGraph = { nodes: [step("a1", "draw", [1]), step("a2", "draw", [2]), step("b1", "draw", [1]), step("b2", "draw", [2]), step("finish", "finish", [1])],
      frames: [{ id: "a", title: "a", address: "item", take: [1], nodes: ["a1", "a2"] }, { id: "b", title: "b", address: "item", take: [2], nodes: ["b1", "b2"] }],
      edges: [{ id: "inside", source: "b1", target: "b2", kinds: ["reads"], consumed: true }, { id: "outside", source: "a1", target: "finish", kinds: ["reads"], consumed: true }] };
    const layout = layoutGraph(graph), model = new DeckSelection(layout);
    const frames = layout.decks.findIndex((deck) => deck.kind === "frame");
    expect(model.members[frames].map((member) => member.bound === true)).toEqual([true, false]);
    expect(model.front(frames, null)).toBe(0);
    expect(model.front(frames, "b1")).toBe(1);
  });
});
