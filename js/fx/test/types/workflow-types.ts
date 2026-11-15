// Workflow authoring types: compiled by the SDK's typecheck, never executed.
import type { GroupStep, NodeStep, Note, VideoPreview } from "../../src/index.js";

export const fileNote: Note = { file: "./notes/setup.md", format: "text" };
export const inlineNote: Note = { text: "## Overview", about: "draw" };
// @ts-expect-error A note has exactly one content source.
export const conflictingNote: Note = { text: "inline", file: "./notes/setup.md" };
// @ts-expect-error A note needs a content source.
export const missingNote: Note = { format: "markdown" };

export const withPoster: VideoPreview = { video: "native", poster: "cover" };
export const withoutPoster: VideoPreview = { video: "native" };
export const clip: NodeStep = { uses: "./nodes/clip.py#clip", preview: withPoster };

// @ts-expect-error a preview must select its video output
export const missingVideo: VideoPreview = { poster: "cover" };
// @ts-expect-error the poster is an output name, not an object or file value
export const filePoster: VideoPreview = { video: "native", poster: { file: "cover" } };
export const group: GroupStep = {
  steps: {},
  // @ts-expect-error only a node step can select its own video outputs
  preview: withPoster,
};
