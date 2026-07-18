#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  batchRunner,
  ensureNodeTool,
  runMatrix,
} from "../lib/runner.mjs";

const CLOSURE_VERSIONS = [
  "20240317.0.0",
  "20250226.0.0",
  "20260629.0.0",
];

const snippets = [
  {
    name: "class-inheritance",
    source: `
class ClosureBase {
  greet() { return "hi"; }
}
class ClosureChild extends ClosureBase {
  constructor(name) {
    super();
    this.name = name;
  }
  label() { return this.greet() + " " + this.name; }
}
window["ClosureChild"] = ClosureChild;
`,
    expected: [
      "class ClosureBase",
      "class ClosureChild extends ClosureBase",
      "super()",
      "label()",
    ],
  },
  {
    name: "iterable-for-of",
    source: `
function closureTotal(items) {
  let sum = 0;
  for (const item of items) {
    sum += item;
  }
  return sum;
}
window["closureTotal"] = closureTotal;
consume(closureTotal([1, 2, 3]));
`,
    execute: true,
    // SIMPLE mode shortens local names and currently emits `let` for a native
    // loop head, so assert either clean loop binding kind instead of spelling.
    expectedAny: [
      ["for (const", " of ", "+="],
      ["for (let", " of ", "+="],
    ],
  },
  {
    name: "iterable-for-of-reused-source",
    source: `
function closureCount(items) {
  for (const item of items) {
    consume(item);
  }
  return items.length;
}
window["closureCount"] = closureCount;
consume(closureCount([1, 2, 3]));
`,
    execute: true,
    expectedAny: [
      ["for (const", " of ", "return", ".length"],
      ["for (let", " of ", "return", ".length"],
    ],
  },
  {
    name: "optional-nullish",
    source: `
function closureAvatar(user) {
  return user.profile?.avatar ?? "none";
}
window["closureAvatar"] = closureAvatar;
consume(closureAvatar({ profile: { avatar: "ada.png" } }));
`,
    execute: true,
    expected: ["?.", "??"],
  },
];

const allSources = snippets.map((snippet) => snippet.source);

function closureBatch(sources, languageOut, compilerVersion) {
  const toolDir = ensureNodeTool(
    `closure-compiler-${compilerVersion}`,
    [`google-closure-compiler@${compilerVersion}`],
  );
  const executable = join(
    toolDir,
    "node_modules",
    ".bin",
    process.platform === "win32" ? "google-closure-compiler.cmd" : "google-closure-compiler",
  );
  const outputs = new Map();
  const tempDir = mkdtempSync(join(tmpdir(), "wakaru-closure-compiler-"));

  try {
    for (const [index, source] of sources.entries()) {
      const inputPath = join(tempDir, `input-${index}.js`);
      const outputPath = join(tempDir, `output-${index}.js`);
      writeFileSync(inputPath, source);
      const result = spawnSync(
        executable,
        [
          `--js=${inputPath}`,
          `--js_output_file=${outputPath}`,
          "--compilation_level=SIMPLE",
          "--language_in=ECMASCRIPT_NEXT",
          `--language_out=${languageOut}`,
          "--warning_level=QUIET",
        ],
        {
          cwd: toolDir,
          encoding: "utf8",
          maxBuffer: 1024 * 1024 * 20,
          shell: process.platform === "win32",
        },
      );
      if (result.error) {
        outputs.set(source, result.error);
      } else if (result.status !== 0) {
        outputs.set(source, new Error(result.stderr || `Closure Compiler exited ${result.status}`));
      } else {
        outputs.set(source, readFileSync(outputPath, "utf8"));
      }
    }
  } finally {
    rmSync(tempDir, { recursive: true, force: true });
  }
  return outputs;
}

const transformers = CLOSURE_VERSIONS.flatMap((compilerVersion) => {
  const es5 = batchRunner(() =>
    closureBatch(allSources, "ECMASCRIPT5", compilerVersion),
  );
  const es2020 = batchRunner(() =>
    closureBatch(allSources, "ECMASCRIPT_2020", compilerVersion),
  );
  return [
    { name: `closure-${compilerVersion}-simple-es5`, run: es5 },
    { name: `closure-${compilerVersion}-simple-es2020`, run: es2020 },
  ];
});

runMatrix({
  name: "closure-compiler",
  snippets,
  transformers,
});
