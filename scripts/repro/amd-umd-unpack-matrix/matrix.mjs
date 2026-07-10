#!/usr/bin/env node

import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, relative, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { ensureNodeTool } from "../lib/runner.mjs";

const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));

const cases = [
  {
    name: "requirejs-optimizer-named-defines",
    tool: "requirejs",
    build: buildRequireJsOptimizer,
    expectedFiles: ["utils/math.js", "app/main.js"],
    expectedNeedles: [
      { file: "app/main.js", text: 'import math from "../utils/math.js";' },
      { file: "app/main.js", text: "console.log(math.add(1, 2));" },
    ],
    rejectedNeedles: [
      { file: "app/main.js", text: "define(" },
      { file: "utils/math.js", text: "define(" },
    ],
  },
  {
    name: "rollup-amd-named-define",
    tool: "rollup",
    build: (dir) => buildRollup(dir, "amd"),
    expectedFiles: ["math-lib.js"],
    expectedNeedles: [
      { file: "math-lib.js", text: "function add(a, b)" },
      { file: "math-lib.js", text: "console.log(add(1, 2));" },
    ],
    rejectedNeedles: [{ file: "math-lib.js", text: "define(" }],
  },
  {
    name: "rollup-amd-anonymous-external",
    tool: "rollup",
    build: buildRollupAnonymousExternal,
    expectedFiles: ["module.js"],
    expectedNeedles: [
      { file: "module.js", text: 'from "math-lib"' },
      { file: "module.js", text: "console.log(total);" },
    ],
    rejectedNeedles: [{ file: "module.js", text: "define(" }],
  },
  {
    name: "rollup-umd-wrapper",
    tool: "rollup",
    build: (dir) => buildRollup(dir, "umd"),
    expectedFiles: ["module.js"],
    expectedNeedles: [
      { file: "module.js", text: "function add(a, b)" },
      { file: "module.js", text: "console.log(add(1, 2));" },
    ],
    rejectedNeedles: [{ file: "module.js", text: "factory" }],
  },
];

const showDetails = process.argv.includes("--details");
const tmpRoot = mkdtempSync(join(tmpdir(), "wakaru-amd-umd-unpack-"));
const failures = [];

try {
  console.log("# AMD and UMD unpack reproduction matrix");
  console.log(`# wakaru: ${wakaruDescription()}`);
  console.log("");
  console.log("| case | tool | unpacked | recovered | notes |");
  console.log("|---|---|---:|---:|---|");

  for (const testCase of cases) {
    const caseDir = join(tmpRoot, testCase.name);
    mkdirSync(caseDir, { recursive: true });

    try {
      const input = testCase.build(caseDir);
      const outputDir = join(caseDir, "out");
      runWakaruUnpack(input, outputDir);
      const modules = collectModules(outputDir);
      const result = checkCase(testCase, modules);
      if (!result.recovered) {
        failures.push({ testCase, input: readFileSync(input, "utf8"), modules, notes: result.notes });
      }
      console.log(
        `| ${testCase.name} | ${testCase.tool} | ${modules.length} | ${
          result.recovered ? "yes" : "no"
        } | ${escapeCell(result.notes)} |`,
      );
    } catch (error) {
      failures.push({ testCase, error });
      console.log(`| ${testCase.name} | ${testCase.tool} | 0 | no | ${escapeCell(error.message)} |`);
    }
  }

  if (showDetails && failures.length > 0) {
    console.log("");
    console.log("## Failure Details");
    for (const failure of failures) {
      console.log("");
      console.log(`### ${failure.testCase.name}`);
      if (failure.error) {
        console.log(failure.error.stack ?? failure.error.message);
        continue;
      }
      console.log("");
      console.log("Input:");
      console.log("```js");
      console.log(failure.input.trim());
      console.log("```");
      for (const [name, code] of failure.modules) {
        console.log("");
        console.log(`Output ${name}:`);
        console.log("```js");
        console.log(code.trim());
        console.log("```");
      }
    }
  }
} finally {
  rmSync(tmpRoot, { recursive: true, force: true });
}

if (failures.length > 0) {
  process.exit(1);
}

function buildRequireJsOptimizer(dir) {
  const toolDir = ensureNodeTool("requirejs-2.3", ["requirejs@2.3.7"]);
  const sourceRoot = join(dir, "src");
  mkdirSync(join(sourceRoot, "utils"), { recursive: true });
  mkdirSync(join(sourceRoot, "app"), { recursive: true });
  writeFileSync(
    join(sourceRoot, "utils", "math.js"),
    `
define(function() {
  function add(a, b) {
    return a + b;
  }
  return { add: add };
});
`,
  );
  writeFileSync(
    join(sourceRoot, "app", "main.js"),
    `
define(["utils/math"], function(math) {
  console.log(math.add(1, 2));
});
`,
  );

  const out = join(dir, "requirejs-bundle.js");
  runChecked("node", [
    join(toolDir, "node_modules", "requirejs", "bin", "r.js"),
    "-o",
    `baseUrl=${sourceRoot}`,
    "name=app/main",
    `out=${out}`,
    "optimize=none",
    "skipModuleInsertion=true",
    "wrap=false",
  ]);
  return out;
}

function buildRollup(dir, format) {
  const toolDir = ensureNodeTool("rollup-4", ["rollup@4"]);
  const sourceRoot = join(dir, "src");
  mkdirSync(sourceRoot, { recursive: true });
  writeFileSync(
    join(sourceRoot, "main.js"),
    `
function add(a, b) {
  return a + b;
}

console.log(add(1, 2));
export { add };
`,
  );

  const out = join(dir, `rollup-${format}.js`);
  runChecked("node", [
    join(toolDir, "node_modules", "rollup", "dist", "bin", "rollup"),
    join(sourceRoot, "main.js"),
    "--format",
    format,
    "--name",
    "MathLib",
    "--amd.id",
    "math-lib",
    "--file",
    out,
    "--silent",
  ]);
  return out;
}

function buildRollupAnonymousExternal(dir) {
  const toolDir = ensureNodeTool("rollup-4", ["rollup@4"]);
  const sourceRoot = join(dir, "src");
  mkdirSync(sourceRoot, { recursive: true });
  writeFileSync(
    join(sourceRoot, "main.js"),
    `
import { add } from "math-lib";

const total = add(1, 2);
console.log(total);
export { total };
`,
  );

  const out = join(dir, "rollup-amd-anonymous-external.js");
  runChecked("node", [
    join(toolDir, "node_modules", "rollup", "dist", "bin", "rollup"),
    join(sourceRoot, "main.js"),
    "--format",
    "amd",
    "--external",
    "math-lib",
    "--file",
    out,
    "--silent",
  ]);
  return out;
}

function runWakaruUnpack(input, outputDir) {
  const configured = process.env.WAKARU;
  if (configured) {
    runChecked(configured, [input, "--unpack", "-o", outputDir]);
    return;
  }

  const debugBinary = join(repoRoot, "target", "debug", process.platform === "win32" ? "wakaru.exe" : "wakaru");
  try {
    runChecked(debugBinary, [input, "--unpack", "-o", outputDir]);
  } catch {
    runChecked("cargo", ["run", "-q", "-p", "wakaru-cli", "--", input, "--unpack", "-o", outputDir], {
      cwd: repoRoot,
    });
  }
}

function collectModules(outputDir) {
  const files = [];
  collectJsFiles(outputDir, outputDir, files);
  return files.sort(([left], [right]) => left.localeCompare(right));
}

function collectJsFiles(root, dir, files) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      collectJsFiles(root, full, files);
    } else if (entry.isFile() && entry.name.endsWith(".js")) {
      files.push([relative(root, full).replaceAll("\\", "/"), readFileSync(full, "utf8")]);
    }
  }
}

function checkCase(testCase, modules) {
  const names = modules.map(([name]) => name);
  const missingFiles = testCase.expectedFiles.filter((name) => !names.includes(name));
  const extraFiles = names.filter((name) => !testCase.expectedFiles.includes(name));
  const missingNeedles = testCase.expectedNeedles.filter(({ file, text }) => !moduleCode(modules, file).includes(text));
  const leakedNeedles = testCase.rejectedNeedles.filter(({ file, text }) => moduleCode(modules, file).includes(text));

  const notes = [];
  if (missingFiles.length > 0) notes.push(`missing files ${missingFiles.join(", ")}`);
  if (extraFiles.length > 0) notes.push(`extra files ${extraFiles.join(", ")}`);
  if (missingNeedles.length > 0) {
    notes.push(`missing ${missingNeedles.map(({ file, text }) => `${file}: ${text}`).join("; ")}`);
  }
  if (leakedNeedles.length > 0) {
    notes.push(`leaked ${leakedNeedles.map(({ file, text }) => `${file}: ${text}`).join("; ")}`);
  }
  return { recovered: notes.length === 0, notes: notes.length === 0 ? "expected modules recovered" : notes.join("; ") };
}

function moduleCode(modules, filename) {
  return modules.find(([name]) => name === filename)?.[1] ?? "";
}

function wakaruDescription() {
  if (process.env.WAKARU) {
    return process.env.WAKARU;
  }
  return join(repoRoot, "target", "debug", process.platform === "win32" ? "wakaru.exe" : "wakaru");
}

function runChecked(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? repoRoot,
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 20,
    shell: options.shell ?? false,
    env: { ...process.env, ...(options.env ?? {}) },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = [result.stderr.trim(), result.stdout.trim()].filter(Boolean).join(" ");
    throw new Error(`${basename(command)} exited ${result.status}: ${detail}`);
  }
  return result.stdout;
}

function escapeCell(value) {
  return value.replaceAll("|", "\\|").replaceAll("\n", " ");
}
