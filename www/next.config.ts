import type { NextConfig } from "next";
import { site } from "./site.config";

const config: NextConfig = {
  output: "export",
  basePath: site.base_path,
  // grida.co strips trailing slashes and proxies /fx here; a slash policy would loop.
  trailingSlash: false,
  // External media belongs to its CDN, never the Next/Vercel optimizer.
  images: { unoptimized: true, remotePatterns: [], domains: [] },
  reactStrictMode: true,
  transpilePackages: ["@grida/fx-web", "@grida/fx-react"],
  // Repository rules already live at the FX root.
  agentRules: false,
};

export default config;
