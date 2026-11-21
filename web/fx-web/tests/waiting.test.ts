import { expect, test } from "bun:test";
import { WaitingReviewController, countdownLabel, parseWaitingRequest, withReviewCards } from "../src/waiting";
import type { CanvasGraph } from "../src/graph";
import { emptyAnswer, encodeAnswer, initialAnswer, scalarEnumOptions } from "../src/answer-draft";

const id = "a".repeat(64);
const request = { id, step: "choose#1", message: "Choose", schema: { type: "object" }, choice_field: "variant",
  choices: { a: { value: "one" }, b: { value: "two" } }, state: { status: "pending" } };

test("a conflict returns the CLI winner and further submissions stop", async () => {
  const posted: RequestInit[] = [];
  const winner = { ...request, state: { status: "answered", channel: "cli", at: "2026-01-01T01:00:00Z", answer: { variant: "a" } } };
  const controller = new WaitingReviewController("/api", (async (url, init) => {
    if (String(url) === "/api/requests") return Response.json({ requests: [request] });
    expect(String(url)).toBe(`/api/requests/${id}/respond`);
    posted.push(init!);
    return Response.json({ request: winner, code: "answer_conflict", error: "Already answered." }, { status: 409 });
  }) as typeof fetch);
  try {
    controller.start();
    await new Promise(resolve => setTimeout(resolve, 0));
    const current = controller.getSnapshot().requests[0];
    const cards = controller.getCards();
    expect(controller.getPendingCount()).toBe(1);
    controller.open(current.step);
    expect(controller.getOpenStep()).toBe(current.step);
    controller.edit(id, '{"note":"reviewed"}');
    controller.choose(current, "b");
    controller.close();
    expect(controller.getOpenStep()).toBe(null);
    expect(controller.getCards()).toBe(cards);
    controller.open(current.step);
    await controller.submit(id);
    expect(JSON.parse(posted[0].body as string)).toEqual({ answer: { note: "reviewed", variant: "b" } });
    expect(new Headers(posted[0].headers).get("x-fx-action")).toBe("respond");
    expect(controller.getSnapshot().requests).toEqual([winner]);
    expect(controller.getPendingCount()).toBe(0);
    await controller.submit(id);
    expect(posted).toHaveLength(1);
  } finally {
    controller.stop();
  }
});

test("a fresh poll can reverse projected expiry, and an in-flight poll preserves a durable receipt", async () => {
  const expired = { ...request, state: { status: "expired", at: "2026-01-01T01:00:00Z" } };
  const answered = { ...request, state: { status: "answered", at: "2026-01-01T00:59:59Z", answer: { variant: "a" }, channel: "viewer" } };
  const stale = Promise.withResolvers<Response>();
  const reads = [Promise.resolve(Response.json({ requests: [expired] })),
    Promise.resolve(Response.json({ requests: [request] })), stale.promise];
  const controller = new WaitingReviewController("/api", (async (_url, init) =>
    init?.method === "POST" ? Response.json(answered) : reads.shift()!) as typeof fetch);
  const poll = async () => {
    controller.stop();
    controller.start();
    await new Promise(resolve => setTimeout(resolve, 0));
  };
  try {
    await poll();
    expect(controller.getPendingCount()).toBe(0);
    await poll();
    expect(controller.getPendingCount()).toBe(1);
    await poll();
    await controller.submit(id, '{"variant":"a"}');
    stale.resolve(Response.json({ requests: [expired] }));
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(controller.getSnapshot().requests).toEqual([answered]);
  } finally {
    controller.stop();
  }
});

test("review actions bind only to the matching concrete canvas instance", () => {
  const deadline = "2026-01-01T01:00:00Z";
  const graph: CanvasGraph = {
    nodes: ["choose['a']#1", "choose['b']#1", "draw#1"].map(source_id => ({
      id: `instance:${source_id}`, source_id, title: source_id, subtitle: "", state: "succeeded", pending: false,
    })),
    edges: [],
  };
  const cards = new Map([
    ["choose['a']#1", { message: "Review A", status: "pending" as const, deadline }],
    ["choose['b']#1", { message: "Review B", status: "expired" as const, deadline }],
  ]);
  const live = withReviewCards(graph, cards);
  expect(live.nodes[0]).toMatchObject({ review_action: "review", review_prompt: "Review A", review_deadline: deadline });
  expect(live.nodes[1]).toMatchObject({ review_action: "view_outcome", review_prompt: "Review B" });
  expect(live.nodes[1].review_deadline).toBeUndefined();
  expect(live.nodes[2]).toBe(graph.nodes[2]);
  expect(withReviewCards(graph, new Map())).toBe(graph);
});

test("countdowns round up a partial second and hold at zero after the deadline", () => {
  expect(countdownLabel(60_001, 0)).toBe("1:01 left");
  expect(countdownLabel(3_605_000, 0)).toBe("1:00:05 left");
  expect(countdownLabel(1, 0)).toBe("0:01 left");
  expect(countdownLabel(1, 1)).toBe("0:00 left");
  expect(countdownLabel(1, 1001)).toBe("0:00 left");
});

test("request IDs cannot select another API path and JSON null is a recorded answer", () => {
  expect(() => parseWaitingRequest({ ...request, id: "../another" })).toThrow();
  const promptless = { id, step: "revision#1", schema: { type: "string" }, state: { status: "pending" } };
  expect(parseWaitingRequest(promptless)).toEqual(promptless);
  expect(() => parseWaitingRequest({ ...promptless, message: null })).toThrow();
  const answered = { ...request, state: { status: "answered", answer: null, channel: "viewer", at: "2026-01-01T00:00:00Z" } };
  expect(parseWaitingRequest(answered)).toEqual(answered);
});

test("enum selection preserves literal JSON values without picking a default", () => {
  const schema = { enum: [0, "", false, null, "0"] };
  expect(initialAnswer(schema)).toBeUndefined();
  const options = scalarEnumOptions(schema)!;
  expect(options.map(option => encodeAnswer(option.draft))).toEqual(["0", '""', "false", "null", '"0"']);
  expect(new Set(options.map(option => option.label)).size).toBe(options.length);
  expect(scalarEnumOptions({ enum: [{ name: "one" }] })).toBeUndefined();
});

test("answer fields preserve exact numeric input and distinguish empty values from incomplete fields", () => {
  const fields = [
    { name: "measure", value: { kind: "number" as const, text: "9007199254740993" } },
    { name: "approved", value: { kind: "boolean" as const, value: false } },
    { name: "note", value: { kind: "string" as const, text: "" } },
    { name: "detail", value: { kind: "null" as const } },
  ];
  expect(encodeAnswer({ kind: "object", fields })).toBe('{"measure":9007199254740993,"approved":false,"note":"","detail":null}');
  expect(encodeAnswer({ kind: "object", fields: [...fields, { name: "detail" }] })).toBeUndefined();
  expect(encodeAnswer({ kind: "array", items: [{ kind: "integer", text: "-" }] })).toBeUndefined();
});

test("an empty required container is an answer, an untouched optional container stays absent", () => {
  const schema = {
    type: "object", required: ["metadata", "tags", "unknown"],
    properties: {
      metadata: { type: "object", required: ["nested"], properties: { nested: { type: "object" } } },
      tags: { type: "array" }, unknown: { type: ["object", "null"] },
      optional: { type: "object", required: ["name"], properties: { name: { type: "string" } } },
    },
  };
  const expected = '{"metadata":{"nested":{}},"tags":[]}';
  expect(encodeAnswer(initialAnswer(schema))).toBe(expected);
  expect(encodeAnswer(emptyAnswer("object", { ...schema, type: ["object", "string"] }))).toBe(expected);
});
