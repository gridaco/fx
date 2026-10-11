import { expect, test } from "bun:test";
import { agentPromptClipboard, agentPromptText } from "../../lib/agent-prompt";

test("copy preserves prose and numbered references with usable asset URLs", () => {
  const image = { label: "this character", href: "/fx/showcases/run/movie/assets/source.png" };
  const brief = { label: "the brief", href: "https://assets.example.com/brief.md" };
  const text = agentPromptText(["Animate ", image, " using ", brief, ".\n\nPlan offline first."]);
  expect(text).toBe("Animate this character [1] using the brief [2].\n\nPlan offline first.");
  const copied = agentPromptClipboard(text, [image, brief], "https://grida.co/fx/workflows/movie-sprite/");
  expect(copied).toContain("[1] this character: https://grida.co/fx/showcases/run/movie/assets/source.png");
  expect(copied).toContain("[2] the brief: https://assets.example.com/brief.md");
  expect(copied).toStartWith(text + "\n\nReference files");
});

test("a prompt without files copies only its prose", () => {
  expect(agentPromptClipboard("Plan offline.", [], "https://grida.co/fx/")).toBe("Plan offline.");
});
