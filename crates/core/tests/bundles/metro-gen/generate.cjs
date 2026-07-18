#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");
const Metro = require("metro");

const root = __dirname;
const outputDir = path.join(root, "dist");

async function generate(name, options) {
  const config = await Metro.loadConfig({
    config: path.join(root, "metro.config.cjs"),
  });
  const result = await Metro.runBuild(config, {
    entry: "src/index.js",
    ...options,
  });
  const reproducibleCode = result.code.replaceAll(root, "<METRO_FIXTURE_ROOT>");
  fs.writeFileSync(
    path.join(outputDir, `${name}.bundle.js`),
    reproducibleCode,
  );
}

async function main() {
  fs.rmSync(outputDir, { recursive: true, force: true });
  fs.mkdirSync(outputDir, { recursive: true });
  await generate("dev", { dev: true, minify: false });
  await generate("min", { dev: false, minify: true });
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
