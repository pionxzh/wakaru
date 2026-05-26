import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "../..");
const crateDir = resolve(repoRoot, "crates/wasm");
const outDir = resolve(repoRoot, "crates/wasm/pkg");

const result = spawnSync(
  "wasm-pack",
  ["build", crateDir, "--target", "web", "--out-dir", outDir, "--release"],
  {
    cwd: repoRoot,
    stdio: "inherit",
  }
);

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
