#!/usr/bin/env node

import { runNodeBatchSync } from "../lib/tool-process.mjs";

import {
  runMatrix, batchRunner, ensureSwcTool, babelPresetEnvBatch,
} from "../lib/runner.mjs";
import { mangleValidator } from "../lib/compare.mjs";

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
    name: "iife-for-of-target",
    bucket: "mangle",
    source: `
(function (a) {
  for (a of items) use(a);
})(0);
`,
    expected: ["let ", "for ("],
    rejected: ["const "],
    skipProfiles: ["all"],
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

// Classes lowered by Babel preset-env (IE 11) before SWC minifies them. SWC
// inlines the single-use `_createClass`, `_defineProperties`, and
// `_classCallCheck` helpers into the class, so these rows track how much of
// that shape class recovery handles. Babel 7.12 predates `_toPropertyKey` in
// `_defineProperties`; 7.29 routes each key through it.
const babelLowerers = [
  { core: "7.12.17", preset: "7.12.17" },
  { core: "7.29.7", preset: "7.29.7" },
];
const classSnippets = [
  {
    name: "class-methods",
    source: `
class Store {
  constructor(items) { this.items = items; }
  get(index) { return this.items[index]; }
  get size() { return this.items.length; }
}
const first = new Store(["a", "b"]);
const second = new Store(["c"]);
use(first.get(1), first.size, second.size);
`,
    expected: ["class ", "get size()"],
    rejected: ["Object.defineProperty(", "Cannot call a class"],
    execute: {},
  },
  {
    name: "class-static",
    source: `
class Parser {
  static parse(text) { return text.trim(); }
  static get version() { return 2; }
}
use(Parser.parse(" a "), Parser.version, Parser.parse("b"));
`,
    expected: ["class ", "static get version()"],
    rejected: ["Object.defineProperty(", "Cannot call a class"],
    execute: {},
  },
  {
    name: "class-extends",
    source: `
class Base {
  constructor(name) { this.name = name; }
  label() { return this.name; }
}
class Child extends Base {
  constructor(name) { super(name); this.kind = "child"; }
  label() { return super.label() + ":" + this.kind; }
}
const first = new Child("a");
const second = new Child("b");
use(first.label(), second.label(), new Base("c").label());
`,
    expected: ["class ", " extends ", "super("],
    rejected: ["Object.defineProperty(", "Cannot call a class", "Object.create("],
    execute: {},
  },
];
for (const lower of babelLowerers) {
  for (const snippet of classSnippets) {
    snippets.push({
      ...snippet,
      name: `babel-${lower.core}-${snippet.name}`,
      bucket: "inline-iife",
      // `inline-iife` turns every other compress default off, so constant
      // folding and dead-code removal never finish the `_createClass`
      // expansion (`if (protoProps) _defineProperties(...)` stays). The
      // default compress options (the `all` profile) fold it away.
      skipProfiles: ["inline-iife"],
      // Same source per Babel version; the comment keeps batch keys distinct.
      source: `// babel ${lower.core}${snippet.source}`,
      lower,
    });
  }
}

// SWC minifier batch
function swcMinifyBatch(sources, options) {
  const toolDir = ensureSwcTool();
  const helperSource = `
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
`;
  return runNodeBatchSync(helperSource, sources, {
    label: "swc-minify-batch.cjs",
    format: "commonjs",
    cwd: toolDir,
    env: { SWC_MINIFY_OPTIONS: JSON.stringify(options) },
  });
}

// Snippets with `lower` reach SWC as the lowerer's output instead of source.
const babelLowered = new Map();
for (const lower of babelLowerers) {
  const sources = snippets.filter((s) => s.lower === lower).map((s) => s.source);
  const lowered = await babelPresetEnvBatch(sources, { ...lower, targets: { ie: "11" } });
  for (const [source, code] of lowered) babelLowered.set(source, code);
}
function minifyInput(source) {
  const lowered = babelLowered.get(source);
  if (lowered instanceof Error) throw lowered;
  return lowered ?? source;
}
const minifyInputs = snippets.map((s) => {
  try {
    return minifyInput(s.source);
  } catch {
    return s.source;
  }
});

// Build per-profile batch runners (lazily cached)
const profileRunners = new Map();
for (const profile of profiles) {
  const lookup = batchRunner(() => swcMinifyBatch(minifyInputs, profile.options));
  profileRunners.set(profile.name, (source) => lookup(minifyInput(source)));
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

runMatrix({
  name: "swc-minifier",
  snippets,
  transformers: [],
  ...mangleValidator(),
});
