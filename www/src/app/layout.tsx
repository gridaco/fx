import type { Metadata } from "next";
import { site } from "../../site.config";
import { SiteNav } from "../components/site-nav";
import { SiteFooter } from "../components/site-footer";
import "./globals.css";

export const metadata: Metadata = {
  metadataBase: new URL(site.origin),
  title: "Grida FX — Grida",
  applicationName: "Grida FX",
  authors: [{ name: "Grida", url: site.origin }],
  publisher: "Grida",
  icons: { icon: `${site.base_path}/brand/grida-mark.svg` },
  robots: {
    index: true,
    follow: true,
    googleBot: { "max-image-preview": "large", "max-snippet": -1, "max-video-preview": -1 },
  },
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return <html lang="en" data-scroll-behavior="smooth">
    <body className="flex min-h-dvh flex-col pt-[68px] font-sans antialiased xl:pt-[100px]">
      <a href="#main" className="sr-only fixed left-4 top-4 z-[60] rounded bg-primary px-4 py-2 text-white focus:not-sr-only">Skip to content</a>
      <SiteNav />
      {children}
      <SiteFooter />
    </body>
  </html>;
}
