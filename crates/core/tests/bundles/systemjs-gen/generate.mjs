#!/usr/bin/env node
// Generates SystemJS test fixtures from the source files in src/.
// Requires Node.js + npm. Generated outputs are checked into the repo so tests
// do not require Node.js.

import { mkdirSync, rmSync } from "node:fs";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const cwd = dirname(fileURLToPath(import.meta.url));

const versions = {
  rollup: "4.29.1",
  babelCli: "7.25.9",
  babelCore: "7.26.0",
  babelSystemjs: "7.25.9",
  swcCli: "0.7.9",
  swcCore: "1.15.3",
  typescript: "5.9.3",
  webpack: "5.103.0",
  webpackCli: "5.1.4",
};

function run(command, args) {
  const result =
    process.platform === "win32"
      ? spawnSync([command, ...args].join(" "), {
          cwd,
          shell: true,
          stdio: "inherit",
        })
      : spawnSync(command, args, {
          cwd,
          stdio: "inherit",
        });

  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

rmSync(`${cwd}/dist`, { recursive: true, force: true });
mkdirSync(`${cwd}/dist/.empty-types`, { recursive: true });

console.log(`=== Rollup ${versions.rollup} ===`);
console.log("  preserve: System.register preserveModules output");
run("npx", [
  "--yes",
  `rollup@${versions.rollup}`,
  "src/entry.js",
  "--format",
  "system",
  "--preserveModules",
  "--dir",
  "dist/preserve",
]);

console.log("");
console.log(`=== Babel ${versions.babelCore} ===`);
console.log("  babel: @babel/plugin-transform-modules-systemjs compiler output");
run("npm", [
  "install",
  "--no-save",
  "--no-package-lock",
  "--ignore-scripts",
  `@babel/cli@${versions.babelCli}`,
  `@babel/core@${versions.babelCore}`,
  `@babel/plugin-transform-modules-systemjs@${versions.babelSystemjs}`,
]);
run("npx", [
  "babel",
  "src",
  "--out-dir",
  "dist/babel",
  "--plugins",
  "@babel/plugin-transform-modules-systemjs",
]);

console.log("");
console.log(`=== SWC ${versions.swcCore} ===`);
console.log("  swc: module.type=systemjs compiler output");
run("npx", [
  "--yes",
  "-p",
  `@swc/cli@${versions.swcCli}`,
  "-p",
  `@swc/core@${versions.swcCore}`,
  "swc",
  "src",
  "-d",
  "dist/swc",
  "--config-file",
  "swc.swcrc",
]);

console.log("");
console.log(`=== TypeScript ${versions.typescript} ===`);
console.log("  tsc: --module system compiler output");
run("npx", [
  "--yes",
  "-p",
  `typescript@${versions.typescript}`,
  "tsc",
  "src-ts/entry.ts",
  "src-ts/dep.ts",
  "--module",
  "system",
  "--target",
  "es2018",
  "--typeRoots",
  "dist/.empty-types",
  "--outDir",
  "dist/tsc",
]);

console.log("");
console.log(`=== Webpack ${versions.webpack} ===`);
console.log("  webpack: output.library.type=system wrapper");
run("npx", [
  "--yes",
  "-p",
  `webpack@${versions.webpack}`,
  "-p",
  `webpack-cli@${versions.webpackCli}`,
  "webpack",
  "--config",
  "webpack.system.config.cjs",
]);

console.log("");
console.log("Done. Outputs in dist/*/");
