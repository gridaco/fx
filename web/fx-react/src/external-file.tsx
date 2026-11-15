import { useSyncExternalStore, type ReactNode } from "react";
import type { ExternalFileController, FileTarget } from "@grida/fx-web";

/** A real browser link by default; the injected controller owns optional external activation. */
export function ExternalFileLink({ target, controller, children, className, title, download }: {
  target: FileTarget;
  controller?: ExternalFileController;
  children: ReactNode;
  className?: string;
  title?: string;
  download?: string;
}) {
  const available = useSyncExternalStore(controller?.subscribe ?? noSubscription, () => controller?.getSnapshot().available ?? false);
  const href = target.kind === "artifact" ? target.artifact.url : null;
  const activate: React.MouseEventHandler = (event) => { controller?.activate(target, event); };
  if (href !== null) return <a href={href} target={download ? undefined : "_blank"} rel="noopener noreferrer" download={download}
    className={className} title={title} onClick={activate}>{children}</a>;
  return available
    ? <button type="button" className={className} title={title} onClick={activate}>{children}</button>
    : <span className={className} title={title}>{children}</span>;
}

const noSubscription = (): (() => void) => () => {};
