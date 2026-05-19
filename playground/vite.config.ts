import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import wasm from "vite-plugin-wasm";
import path from "node:path";

export default defineConfig({
  plugins: [react(), wasm()],
  resolve: {
    alias: {
      "wakaru-wasm": path.resolve(__dirname, "../crates/wasm/pkg"),
    },
  },
  worker: {
    plugins: () => [wasm()],
  },
  server: {
    fs: {
      allow: [path.resolve(__dirname, "..")],
    },
  },
  build: {
    target: "esnext",
  },
  optimizeDeps: {
    exclude: ["wakaru-wasm"],
  },
});
