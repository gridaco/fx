import { describe, expect, test } from "bun:test";
import { WorkflowNavigation } from "../src/navigation";

describe("workflow navigation", () => {
  test("keeps repeated occurrences distinct and restores selection at every level", () => {
    const navigation = new WorkflowNavigation();
    const scopes = [
      { id: "import#1", title: "First import", parent: null },
      { id: "import#2", title: "Second import", parent: null },
      { id: "nested#1.1", title: "Nested", parent: "import#1" },
    ];
    const before = JSON.stringify(scopes);
    navigation.setScopes(scopes, "Root");
    navigation.select("scope:import#1");
    expect(navigation.open("nested#1.1")).toBeFalse();
    expect(navigation.open("unknown")).toBeFalse();
    expect(navigation.open("import#1")).toBeTrue();
    navigation.select("instance:local#1");
    expect(navigation.open("nested#1.1")).toBeTrue();
    navigation.select("instance:inside#1");
    expect(navigation.getSnapshot().breadcrumbs.map((item) => item.id)).toEqual([null, "import#1", "nested#1.1"]);
    expect(navigation.goTo("import#2")).toBeFalse();
    expect(navigation.goTo("import#1")).toBeTrue();
    expect(navigation.getSnapshot().selected).toBe("instance:local#1");
    navigation.goTo(null);
    expect(navigation.getSnapshot().selected).toBe("scope:import#1");
    navigation.open("import#2");
    expect(navigation.getSnapshot().selected).toBeNull();
    navigation.goTo(null);
    navigation.open("import#1");
    navigation.open("nested#1.1");
    expect(navigation.getSnapshot().selected).toBe("instance:inside#1");
    expect(JSON.stringify(scopes)).toBe(before);
  });

  test("refresh retains valid scopes and returns to the nearest surviving ancestor", () => {
    const navigation = new WorkflowNavigation();
    const scopes = [{ id: "a", title: "A", parent: null }, { id: "b", title: "B", parent: "a" }];
    navigation.setScopes(scopes, "Root");
    navigation.open("a"); navigation.open("b");
    navigation.setScopes(scopes, "Renamed");
    expect(navigation.getSnapshot().scope).toBe("b");
    expect(navigation.getSnapshot().breadcrumbs[0].title).toBe("Renamed");
    navigation.setScopes(scopes.slice(0, 1), "Root");
    expect(navigation.getSnapshot().scope).toBe("a");
    navigation.setScopes([], "Legacy flat view");
    expect(navigation.getSnapshot()).toEqual({ scope: null, breadcrumbs: [{ id: null, title: "Legacy flat view" }], selected: null });
  });
});
