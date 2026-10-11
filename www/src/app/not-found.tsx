import type { Metadata } from "next";
import Link from "next/link";

export const metadata: Metadata = {
  title: "Page not found — Grida FX — Grida",
  robots: { index: false, follow: false },
};

export default function NotFound() {
  return <main id="main" className="mx-auto w-full max-w-6xl px-6 py-24 sm:px-10">
    <p className="font-mono text-xs text-muted-foreground">404</p>
    <h1 className="mt-4 text-3xl font-semibold tracking-tight">Page not found.</h1>
    <Link href="/" className="mt-6 inline-block text-sm underline underline-offset-4">Return to Grida FX</Link>
  </main>;
}
