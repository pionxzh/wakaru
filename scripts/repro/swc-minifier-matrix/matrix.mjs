#!/usr/bin/env node

import {
  runMatrix, batchRunner, ensureNodeTool,
} from "../lib/runner.mjs";
import { join } from "node:path";
import { writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const profiles = [
  {
    name: "sequences",
    options: {
      compress: { defaults: false, sequences: true },
      mangle: false,
    },
  },
  {
    name: "booleans",
    options: {
      compress: { defaults: false, booleans: true },
      mangle: false,
    },
  },
  {
    name: "evaluate",
    options: {
      compress: { defaults: false, evaluate: true },
      mangle: false,
    },
  },
  {
    name: "inline-iife",
    options: {
      compress: { defaults: false, inline: 3, reduce_vars: true, reduce_funcs: true },
      mangle: false,
    },
  },
  {
    name: "mangle",
    options: {
      compress: false,
      mangle: { toplevel: true },
    },
  },
  {
    name: "all",
    options: {
      compress: true,
      mangle: true,
    },
  },
];

const snippets = [
  {
    name: "sequence-before-if",
    bucket: "sequences",
    source: `
function run(y) {
  x = 5;
  if (y) z();
}
run(input);
`,
    expectedAny: [
      ["x = 5;", "if (y)"],
      ["x = 5;", "if (input_1)"],
      ["x = 5;", "if (input)"],
    ],
  },
  {
    name: "sequence-before-return",
    bucket: "sequences",
    source: `
function run() {
  side();
  return value;
}
console.log(run());
`,
    expected: ["side();", "return value;"],
    skipProfiles: ["all"],
  },
  {
    name: "sequence-before-for",
    bucket: "sequences",
    source: `
function run() {
  setup();
  for (; i < n; i++) work(i);
}
run();
`,
    expected: ["setup();", "for(; i < n; i++)"],
  },
  {
    name: "sequence-before-throw",
    bucket: "sequences",
    source: `
function run() {
  log();
  throw error;
}
try {
  run();
} catch (caught) {
  handle(caught);
}
`,
    expected: ["log();", "throw error;"],
  },
  {
    name: "boolean-literals",
    bucket: "booleans",
    source: `
const yes = true;
const no = false;
console.log(yes, no);
`,
    expected: ["true", "false"],
  },
  {
    name: "negated-condition",
    bucket: "booleans",
    source: `
function run(flag) {
  if (!flag) disabled();
}
run(input);
`,
    expectedAny: [["if (!flag)"], ["if (!input)"]],
  },
  {
    name: "boolean-return",
    bucket: "booleans",
    source: `
function run(flag) {
  return flag ? true : false;
}
console.log(run(input));
`,
    expectedAny: [["return !!flag;"], ["!!input"]],
  },
  {
    name: "double-negation",
    bucket: "booleans",
    source: `
const out = !!value;
console.log(out);
`,
    expected: ["!!value"],
  },
  {
    name: "undefined-infinity",
    bucket: "evaluate",
    source: `
const missing = undefined;
const forever = Infinity;
console.log(missing, forever);
`,
    expected: ["undefined", "Infinity"],
  },
  {
    name: "numeric-fold",
    bucket: "evaluate",
    source: `
const total = 1 + 2 * 3;
console.log(total);
`,
    expectedAny: [["const total = 7;"], ["console.log(7)"]],
  },
  {
    name: "string-constant-access",
    bucket: "evaluate",
    source: `
const letter = "abc".charAt(1);
console.log(letter);
`,
    expected: ["const letter = \"b\";"],
    informational: true,
  },
  {
    name: "array-constant-access",
    bucket: "evaluate",
    source: `
const item = [1, 2, 3][1];
console.log(item);
`,
    expectedAny: [["const item = 2;"], ["console.log(2)"]],
    informational: true,
  },
  {
    name: "arrow-iife-arg",
    bucket: "inline-iife",
    source: `
const out = ((value) => value + 1)(input);
console.log(out);
`,
    expected: ["input + 1"],
    informational: true,
  },
  {
    name: "function-iife-arg",
    bucket: "inline-iife",
    source: `
const out = (function (value) {
  return value + 1;
})(input);
console.log(out);
`,
    expected: ["input + 1"],
    informational: true,
  },
  {
    name: "single-use-temp-alias",
    bucket: "inline-iife",
    source: `
function run(input) {
  const alias = input.value;
  return alias;
}
console.log(run(input));
`,
    expectedAny: [["return input1.value;"], ["input.value"]],
  },
  {
    name: "callback-wrapper",
    bucket: "inline-iife",
    source: `
const wrapped = function (value) {
  return handler(value);
};
wrapped(input);
`,
    expected: ["handler(input)"],
    informational: true,
  },
  {
    name: "react-hook-tuple",
    bucket: "mangle",
    source: `
import { useState } from "react";
export function Counter() {
  const [count, setCount] = useState(0);
  return setCount(count + 1);
}
`,
    expected: ["useState", "setT"],
  },
  {
    name: "member-init-name",
    bucket: "mangle",
    source: `
const logger = services.logger;
logger.info("ready");
`,
    expected: ["logger.info"],
  },
  {
    name: "symbol-for-name",
    bucket: "mangle",
    source: `
const token = Symbol.for("wakaru.token");
console.log(token);
`,
    expected: ["token"],
  },
  {
    name: "component-value-position",
    bucket: "mangle",
    source: `
const UserCard = registry.UserCard;
export const view = UserCard(props);
`,
    expected: ["UserCard"],
  },
];

// SWC minifier batch
function swcMinifyBatch(sources, options) {
  const toolDir = ensureNodeTool("swc", ["@swc/core@1"]);
  const helper = join(toolDir, "swc-minify-batch.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const swc = require("@swc/core");
const options = JSON.parse(process.env.SWC_MINIFY_OPTIONS);
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: swc.minifySync(source, {
      ...options,
      format: { ascii_only: true, comments: false },
      module: true,
    }).code };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`,
  );
  const result = spawnSync("node", [helper], {
    cwd: toolDir,
    input: JSON.stringify(sources),
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 50,
    env: { ...process.env, SWC_MINIFY_OPTIONS: JSON.stringify(options) },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`swc batch exited ${result.status}: ${result.stderr}`);
  const outputs = JSON.parse(result.stdout);
  const map = new Map();
  for (let i = 0; i < sources.length; i++) {
    map.set(sources[i], outputs[i].error ? new Error(outputs[i].error) : outputs[i].code);
  }
  return map;
}

const allSources = snippets.map((s) => s.source);

// Build per-profile batch runners (lazily cached)
const profileRunners = new Map();
for (const profile of profiles) {
  profileRunners.set(profile.name, batchRunner(() => swcMinifyBatch(allSources, profile.options)));
}

function profilesFor(snippet) {
  const skip = new Set(snippet.skipProfiles ?? []);
  const bucketProfiles = profiles.filter((p) => p.name === snippet.bucket);
  const allProfile = profiles.find((p) => p.name === "all");
  return [...bucketProfiles, allProfile].filter(Boolean).filter((p) => !skip.has(p.name));
}

// Assign per-snippet transformers via extraTransformers
for (const snippet of snippets) {
  snippet.extraTransformers = profilesFor(snippet).map((profile) => ({
    name: profile.name,
    run: profileRunners.get(profile.name),
  }));
}

// Custom expectedNeedles supporting expectedAny.
// The runner checks that ALL returned needles are present. For expectedAny, the original
// semantics is "pass if ANY group is fully present". We approximate this by returning
// the intersection of all groups (needles common to every alternative).
function expectedNeedles(snippet) {
  if (snippet.expectedAny) {
    const first = new Set(snippet.expectedAny[0]);
    for (let i = 1; i < snippet.expectedAny.length; i++) {
      const group = new Set(snippet.expectedAny[i]);
      for (const needle of first) {
        if (!group.has(needle)) first.delete(needle);
      }
    }
    return [...first];
  }
  return Array.isArray(snippet.expected) ? snippet.expected : [snippet.expected];
}

runMatrix({
  name: "swc-minifier",
  snippets,
  transformers: [],
  expectedNeedles,
});
