import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  base: "./",
  envDir: false,
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      "/api": { target: "http://127.0.0.1:8080", changeOrigin: true },
      "^/p/[a-f0-9]{64}/(?:runs|plans)/[a-f0-9]{64}/api(?:/|$)": {
        target: "http://127.0.0.1:8080", changeOrigin: true,
      },
    },
  },
});
