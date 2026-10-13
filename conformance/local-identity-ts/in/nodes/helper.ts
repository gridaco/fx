// A project module the node types import, which imports another: both are in their closure.

import { trimmed } from "../lib/words.ts";

export function frame(text: string): string {
  return `[${trimmed(text)}]`;
}
