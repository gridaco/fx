export type AgentPromptReference = { label: string; href: string; image?: string };
export type AgentPromptPart = string | AgentPromptReference;

/** Keep clipboard prose separate from chip badges and other presentation text. */
export function agentPromptText(parts: readonly AgentPromptPart[]): string {
  let reference = 0;
  return parts.map((part) => typeof part === "string" ? part : `${part.label} [${++reference}]`).join("");
}

export function agentPromptClipboard(text: string, references: readonly AgentPromptReference[], base: string): string {
  if (references.length === 0) return text;
  const files = references.map((reference, index) => `[${index + 1}] ${reference.label}: ${new URL(reference.href, base).href}`);
  return `${text}\n\nReference files (open these links, or attach your own replacements):\n${files.join("\n")}`;
}
