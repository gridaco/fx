import { useLayoutEffect, useRef, type ReactNode } from "react";
import { VisualPreviewController } from "@grida/fx-web";

/**
 * Internal visual-media contract: every visual renderer uses this shell and gets
 * fullscreen by default. Only renderers with their own native fullscreen controls opt out.
 */
export function VisualPreview({ children, fullscreen = "managed" }: { children: ReactNode; fullscreen?: "managed" | "native" }) {
  const root = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (fullscreen === "native" || !root.current) return;
    const controller = new VisualPreviewController(root.current);
    return () => controller.dispose();
  }, [fullscreen]);
  return fullscreen === "native" ? children : <div ref={root}><div className="fx-media-content">{children}</div></div>;
}
