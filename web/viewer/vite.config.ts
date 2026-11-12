import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  base: "./",
  envDir: false,
  plugins: [react(), tailwindcss()],
  build: {
    license: { fileName: "assets/licenses.md" },
    // The local readiness script must remain an HTTP asset under script-src 'self'.
    assetsInlineLimit: (path) => path.endsWith("model-meshopt-ready.js") ? false : undefined,
    rollupOptions: {
      output: {
        // Three.js loads decoder siblings by these fixed names. They remain lazy assets.
        assetFileNames: (asset) => asset.names.some(name => /^(?:draco_decoder|draco_wasm_wrapper|basis_transcoder)\.(?:js|wasm)$/.test(name))
          ? "assets/model-decoders/[name][extname]" : "assets/[name]-[hash][extname]",
      },
    },
  },
  server: {
    proxy: {
      "/api": { target: "http://127.0.0.1:8080", changeOrigin: true },
      "^/p/[a-f0-9]{64}/(?:runs|plans)/[a-f0-9]{64}/api(?:/|$)": {
        target: "http://127.0.0.1:8080", changeOrigin: true,
      },
    },
  },
});
