import { describe, expect, test } from "bun:test";
import { ServiceIndexProjection, parseServiceIndex, viewerEntryUrl, type ServiceEntry, type ServiceIndex } from "../src/index";

const project = "a".repeat(64);
const workflow = { id: "recolor", source: "workflows/recolor.yaml" };

function entry(serial: number, options: Partial<ServiceEntry> = {}): ServiceEntry {
  const id = serial.toString(16).padStart(64, "0"), kind = options.kind ?? "run";
  return {
    id, kind, title: "Recolor", url: viewerEntryUrl(project, kind, id), state: kind === "run" ? "succeeded" : "planned",
    workflow, created_at: `2026-10-08T00:00:${String(serial).padStart(2, "0")}.000Z`, ...options,
  };
}

function catalog(entries: ServiceEntry[]): ServiceIndex {
  return { kind: "fx-service-index-v1", project_id: project, title: "Local project", entries };
}

describe("recorded project metadata", () => {
  test("accepts optional metadata and retains extension fields and legacy records", () => {
    const value = catalog([entry(1, { name: "character_1_rig_ready" }), entry(2, { kind: "plan" })]);
    Object.assign(value.entries[0], { future: "preserved" });
    expect(parseServiceIndex(value)).toBe(value);
    const legacy = entry(3); delete legacy.workflow; delete legacy.created_at;
    expect(parseServiceIndex(catalog([legacy])).entries[0]).toBe(legacy);
    for (const created_at of ["2026-10-08T00:00:00Z", "2026-10-08T00:00:00.123456789Z", "2024-02-29T23:59:59.000Z"]) {
      expect(parseServiceIndex(catalog([entry(1, { created_at })]))).toBeDefined();
    }
    expect(parseServiceIndex(catalog([entry(1, { workflow: { id: "recolor", source: "../shared/recolor.yaml" } })]))).toBeDefined();
    expect(parseServiceIndex(catalog([entry(1, { workflow: { id: "recolor", source: "workflows/recolor.py:build" } })]))).toBeDefined();
  });

  test("rejects malformed timestamps instead of normalizing impossible dates", () => {
    for (const created_at of ["", "2026-02-29T00:00:00Z", "2026-02-31T00:00:00Z", "2026-13-01T00:00:00Z",
      "2026-10-08T24:00:00Z", "2026-10-08T00:60:00Z", "2026-10-08T00:00:60Z",
      "2026-10-08T00:00:00+00:00", "2026-10-08", "2026-10-08 00:00:00Z", null, 10]) {
      const invalid = entry(1); Object.assign(invalid, { created_at });
      expect(() => parseServiceIndex(catalog([invalid]))).toThrow("invalid entry");
    }
  });

  test("names are valid run-only metadata and source identity stays portable", () => {
    for (const name of ["", "-first", "with space", "folder/name", "x".repeat(65), null, 1]) {
      const invalid = entry(1); Object.assign(invalid, { name });
      expect(() => parseServiceIndex(catalog([invalid]))).toThrow("invalid entry");
    }
    expect(() => parseServiceIndex(catalog([entry(1, { kind: "plan", name: "baseline" })]))).toThrow("invalid entry");
    for (const source of ["", "/private/source.yaml", "\\private\\source.yaml", "C:\\private\\source.yaml", "C:relative.yaml", "file:///private/source.yaml",
      "folder\\source.yaml", "https://example.invalid/source.yaml", "custom+scheme://host/source", "folder\nsource.yaml", "folder\0source.yaml", "folder\x7fsource.yaml"]) {
      expect(() => parseServiceIndex(catalog([entry(1, { workflow: { id: "recolor", source } })]))).toThrow("invalid entry");
    }
    for (const value of [null, "recolor", { id: "recolor" }, { id: "", source: "recolor.yaml" }, { id: "bad\nid", source: "recolor.yaml" }]) {
      const invalid = entry(1); Object.assign(invalid, { workflow: value });
      expect(() => parseServiceIndex(catalog([invalid]))).toThrow("invalid entry");
    }
  });
});

describe("workflow index projection", () => {
  test("opens the newest-created run including failures, with plans in the same history", () => {
    const first = entry(1, { name: "baseline" }), latest = entry(2, { name: "variant", state: "failed", title: "Recolor revised" });
    const saved = entry(3, { kind: "plan" });
    const value = catalog([saved, first, latest]), before = [...value.entries];
    const groups = new ServiceIndexProjection(value).groups;
    expect(groups).toHaveLength(1);
    expect(groups[0]).toMatchObject({ title: "Recolor revised", primary: latest, latest_known: true, show_source: false });
    expect(groups[0].runs).toEqual([latest, first]);
    expect(groups[0].plans).toEqual([saved]);
    expect(value.entries).toEqual(before);
  });

  test("keeps older recorded running or unfinished runs visible outside collapsed history", () => {
    const running = entry(1, { state: "running", name: "long_run" }), unfinished = entry(2, { state: "unfinished" });
    const latest = entry(3, { state: "failed" });
    const group = new ServiceIndexProjection(catalog([running, latest, unfinished])).groups[0];
    expect(group.primary).toBe(latest);
    expect(group.unfinished_runs).toEqual([unfinished, running]);
    const newerRunning = { ...latest, state: "running" };
    expect(new ServiceIndexProjection(catalog([running, newerRunning])).groups[0].unfinished_runs).toEqual([running]);
  });

  test("recorded ID and exact source group definition and input changes, never titles or hashes", () => {
    const initial = entry(1, { title: "Old title" }), revised = entry(2, { title: "New title" });
    Object.assign(initial, { plan: "old digest", inputs: { hue: 10 } });
    Object.assign(revised, { plan: "new digest", inputs: { hue: 20 } });
    const otherSource = entry(3, { title: "New title", workflow: { ...workflow, source: "other/recolor.yaml" } });
    const otherId = entry(4, { title: "New title", workflow: { ...workflow, id: "other" } });
    const groups = new ServiceIndexProjection(catalog([initial, revised, otherSource, otherId])).groups;
    expect(groups).toHaveLength(3);
    const original = groups.find((group) => group.primary === revised)!;
    expect(original.runs).toEqual([revised, initial]);
    expect(groups.every((group) => group.show_source)).toBeTrue();
    const differentProject = new ServiceIndexProjection({ ...catalog([initial]), project_id: "b".repeat(64) });
    expect(differentProject.groups[0].key).not.toBe(new ServiceIndexProjection(catalog([initial])).groups[0].key);
  });

  test("a resume updates state but immutable creation time does not reorder it as a fresh run", () => {
    const first = entry(1, { name: "baseline", state: "unfinished" }), later = entry(2);
    expect(new ServiceIndexProjection(catalog([first, later])).groups[0].primary).toBe(later);
    const resumed = { ...first, state: "succeeded" };
    const group = new ServiceIndexProjection(catalog([resumed, later])).groups[0];
    expect(group.primary).toBe(later);
    expect(group.runs).toEqual([later, resumed]);
  });

  test("plan-only workflows open their newest saved plan", () => {
    const first = entry(1, { kind: "plan" }), latest = entry(2, { kind: "plan" });
    const group = new ServiceIndexProjection(catalog([first, latest])).groups[0];
    expect(group.primary).toBe(latest); expect(group.plans).toEqual([latest, first]);
    expect(group.runs).toEqual([]); expect(group.unfinished_runs).toEqual([]);
  });

  test("legacy entries remain separate without fabricated creation times or latest claims", () => {
    const legacyRun = entry(1), legacyPlan = entry(1, { kind: "plan" });
    delete legacyRun.workflow; delete legacyPlan.workflow; delete legacyRun.created_at; delete legacyPlan.created_at;
    const groups = new ServiceIndexProjection(catalog([legacyRun, legacyPlan])).groups;
    expect(groups).toHaveLength(2);
    expect(groups.every((group) => !group.latest_known && group.primary.created_at === undefined)).toBeTrue();
    // A workflow ID/source that resembles a legacy tuple cannot share its namespace.
    const matchingText = entry(2, { workflow: { id: "run", source: legacyRun.id } });
    expect(new ServiceIndexProjection(catalog([legacyRun, matchingText])).groups).toHaveLength(2);
    const undated = entry(2); delete undated.created_at;
    const mixed = new ServiceIndexProjection(catalog([undated, entry(1)])).groups[0];
    expect(mixed.primary.id).toBe(entry(1).id); expect(mixed.latest_known).toBeFalse();
  });

  test("UTC precision, equal-time ties and input permutation have deterministic ordering", () => {
    const first = entry(1, { created_at: "2026-10-08T00:00:00.100000001Z" });
    const later = entry(2, { created_at: "2026-10-08T00:00:00.100000002Z" });
    expect(new ServiceIndexProjection(catalog([first, later])).groups[0].primary).toBe(later);
    const same = entry(3, { created_at: later.created_at });
    const original = new ServiceIndexProjection(catalog([same, later, first]));
    const reordered = new ServiceIndexProjection(catalog([first, later, same]));
    expect(original.groups[0].runs).toEqual(reordered.groups[0].runs);
    const two = entry(4, { workflow: { id: "other", source: "other.yaml" } });
    expect(new ServiceIndexProjection(catalog([later, two])).groups[0].primary).toBe(two);
    expect(new ServiceIndexProjection(catalog([])).groups).toEqual([]);
  });
});
