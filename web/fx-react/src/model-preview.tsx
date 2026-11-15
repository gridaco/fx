import { useLayoutEffect, useRef } from "react";
import { ModelPreviewController, type Artifact, type ExternalFileController } from "@grida/fx-web";

/** React mounts the independently owned model inspector, retaining it across record polls. */
export function ModelPreview({ artifact, externalFiles }: { artifact: Artifact; externalFiles?: ExternalFileController }) {
  const root = useRef<HTMLDivElement>(null);
  const { digest, kind, name, size, available, url } = artifact;
  useLayoutEffect(() => {
    if (!root.current) return;
    const controller = new ModelPreviewController(root.current, { digest, kind, name, size, available, url }, { externalFiles });
    return () => controller.dispose();
  }, [digest, kind, name, size, available, url, externalFiles]);
  return <div ref={root} />;
}
