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
  webpack(config) {
    config.module.rules.push({
      dependency: "url",
      test: /(?:draco_decoder|draco_wasm_wrapper|basis_transcoder)\.(?:js|wasm)$/,
      type: "asset/resource",
      generator: { filename: "static/media/model-decoders/[name][ext]" },
    });
    return config;
  },
  // Repository rules already live at the FX root.
  agentRules: false,
};

export default config;
