"use client";

import { useEffect, useRef, type ReactNode } from "react";
import { LandingMotionController } from "./landing-motion-controller";

/** React only mounts the class; page content is rendered by server components. */
export function LandingMotion({ children, className }: { children: ReactNode; className: string }) {
  const root = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const controller = new LandingMotionController(root.current);
    return () => controller.dispose();
  }, []);

  return <main id="main" ref={root} className={className}>{children}</main>;
}
