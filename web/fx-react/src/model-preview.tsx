import { useLayoutEffect, useRef } from "react";
import { ModelPreviewController, type Artifact } from "@grida/fx-web";

/** React mounts the independently owned model inspector, retaining it across record polls. */
export function ModelPreview({ artifact }: { artifact: Artifact }) {
  const root = useRef<HTMLDivElement>(null);
  const { digest, kind, name, size, available, url } = artifact;
  useLayoutEffect(() => {
    if (!root.current) return;
    const controller = new ModelPreviewController(root.current, { digest, kind, name, size, available, url });
    return () => controller.dispose();
  }, [digest, kind, name, size, available, url]);
  return <div ref={root} />;
}
