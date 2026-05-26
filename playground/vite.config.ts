import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import wasm from "vite-plugin-wasm";
import path from "node:path";
import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";

function getWakaruVersion() {
  const cargoToml = readFileSync(path.resolve(__dirname, "../Cargo.toml"), "utf8");
  return cargoToml.match(/^version = "([^"]+)"/m)?.[1] ?? "0.0.0";
}

function getGitHash() {
  try {
    return execSync("git rev-parse --short=8 HEAD", {
      cwd: path.resolve(__dirname, ".."),
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return "unknown";
  }
}

export default defineConfig(({ command }) => ({
  base: command === "build" ? "/playground/" : "/",
  plugins: [react(), wasm()],
  define: {
    "import.meta.env.VITE_WAKARU_VERSION": JSON.stringify(getWakaruVersion()),
    "import.meta.env.VITE_WAKARU_GIT_HASH": JSON.stringify(getGitHash()),
  },
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
}));
