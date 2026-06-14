#!/usr/bin/env node

import {
  runMatrix, batchRunner, babelMultiPluginBatch, babelPresetEnvBatch,
  tscBatch, swcBatch, esbuildBatch, withTerserVariants,
} from "../lib/runner.mjs";

const snippets = [
  {
    name: "async-simple-await",
    source: "async function load_user(app_id) {\n  await fetch_user(app_id);\n}\n",
    expected: ["async function load_user(app_id)", "await fetch_user(app_id)"],
  },
  {
    name: "async-return-value",
    source:
      "async function load_user(app_id) {\n  const response = await fetch_user(app_id);\n  const data = await response.json();\n  return data;\n}\n",
    expected: ["async function load_user(app_id)", "await fetch_user(app_id)", "await response.json()", "return data"],
    expectedAny: [
      ["async function load_user(app_id)", "await fetch_user(app_id)", "await response.json()", "return data"],
      ["async function load_user(app_id)", "await fetch_user(app_id)", "return await response.json()"],
      ["async function", "await fetch_user(", "return await", ".json()"],
    ],
  },
  {
    name: "async-try-catch",
    source:
      "async function load_user(app_id) {\n  try {\n    return await fetch_user(app_id);\n  } catch (error) {\n    return fallback_user(error);\n  }\n}\n",
    expected: ["async function load_user(app_id)", "try", "return await fetch_user(app_id)", "catch"],
  },
  {
    name: "async-try-finally-await",
    source:
      "async function save_record(record) {\n  const lock = await acquire_lock(record.id);\n  try {\n    const payload = await prepare_record(record);\n    return await commit_record(payload);\n  } finally {\n    await lock.release();\n  }\n}\n",
    expected: [
      "async function save_record(record)",
      "await acquire_lock(record.id)",
      "try",
      "await prepare_record(record)",
      "return await commit_record(payload)",
      "finally",
      "await lock.release()",
    ],
  },
  {
    name: "async-double-await",
    source: "async function resolve_deep(promise) {\n  return await await promise;\n}\n",
    expected: ["async function resolve_deep(promise)", "await await promise"],
    expectedAny: [
      ["async function resolve_deep(promise)", "await await promise"],
      ["async function resolve_deep(promise)", "return await await promise"],
      ["async function", "await await"],
    ],
  },
  {
    name: "async-simple-loop",
    source:
      "async function process_items(items) {\n  const results = [];\n  for (let index = 0; index < items.length; index++) {\n    results.push(await transform_item(items[index]));\n  }\n  return results;\n}\n",
    expected: [
      "async function process_items(items)",
      "for",
      "await transform_item(",
      "return results",
    ],
    expectedAny: [
      [
        "async function process_items(items)",
        "for (let index = 0",
        "await transform_item(items[index])",
        "return results",
      ],
      [
        "async function process_items(items)",
        "for(;",
        "await transform_item(",
        "return results",
      ],
      [
        "async function",
        "for",
        "await transform_item(",
        "return",
      ],
    ],
  },
  {
    name: "async-loop-try-catch",
    source:
      "async function collect_enabled(items) {\n  const output = [];\n  for (let index = 0; index < items.length; index++) {\n    const item = items[index];\n    if (!item.enabled) {\n      continue;\n    }\n    try {\n      output.push(await fetch_item(item.id));\n    } catch (error) {\n      output.push(await recover_item(item, error));\n    }\n  }\n  return output;\n}\n",
    expected: [
      "async function collect_enabled(items)",
      "for (let index = 0",
      "const item = items[index]",
      "continue",
      "try",
      "await fetch_item(item.id)",
      "catch",
      "await recover_item(item, error)",
      "return output",
    ],
    expectedAny: [
      [
        "async function collect_enabled(items)",
        "for (let index = 0",
        "const item = items[index]",
        "continue",
        "try",
        "await fetch_item(item.id)",
        "catch",
        "await recover_item(item, error)",
        "return output",
      ],
      [
        "async function collect_enabled(items)",
        "for (const item of items)",
        "try",
        "await fetch_item(item.id)",
        "catch",
        "await recover_item(item, error)",
        "return output",
      ],
      [
        "async function collect_enabled(items)",
        "for(; index < items.length; index++)",
        "item = items[index]",
        "continue",
        "try",
        "await fetch_item(item.id)",
        "catch",
        "await recover_item(item, error",
        "return output",
      ],
    ],
  },
  {
    name: "async-destructuring-default-await",
    source:
      "async function normalize_user(input) {\n  const source = input == null ? await load_user() : input;\n  const { id, profile: { name } = {}, tags: [primary, , backup] = [] } = source;\n  const resolved_backup = backup == null ? await load_backup(id) : backup;\n  const meta = await load_meta(id);\n  return { id, name, primary, backup: resolved_backup, meta };\n}\n",
    expected: [
      "async function normalize_user(input)",
      "const source = input == null ? await load_user() : input",
      "profile: { name }",
      "tags: [primary, , backup]",
      "await load_backup(id)",
      "await load_meta(id)",
      "return {",
      "backup: resolved_backup",
    ],
    expectedAny: [
      [
        "async function normalize_user(input)",
        "const source = input == null ? await load_user() : input",
        "profile: { name }",
        "tags: [primary, , backup]",
        "await load_backup(id)",
        "await load_meta(id)",
        "return {",
        "backup: resolved_backup",
      ],
      [
        "async function normalize_user(input)",
        "const source = input ?? await load_user()",
        "profile: { name }",
        "tags: [primary, , backup]",
        "await load_backup(id)",
        "await load_meta(id)",
        "return {",
        "backup",
      ],
      [
        "async function normalize_user(input)",
        "profile: { name }",
        "tags: [primary, , backup]",
        "await load_backup(id)",
        "await load_meta(id)",
        "return {",
        "backup: resolved_backup",
      ],
    ],
  },
  {
    name: "async-arrow",
    source: "const load_user = async (app_id) => await fetch_user(app_id);\nuse(load_user);\n",
    expected: ["async (app_id)", "await fetch_user(app_id)"],
    expectedAny: [
      ["async (app_id)", "await fetch_user(app_id)"],
      ["async function(app_id)", "await fetch_user(app_id)"],
      ["async function load_user(app_id)", "await fetch_user(app_id)"],
    ],
  },
  {
    name: "async-arrow-nested-awaits",
    source:
      "const run_pipeline = async (source) => {\n  const steps = await load_steps(source);\n  return steps.map(async (step) => await step.run(source));\n};\nuse(run_pipeline);\n",
    expected: [
      "const run_pipeline = async (source)",
      "await load_steps(source)",
      "steps.map(async (step)",
      "await step.run(source)",
    ],
    expectedAny: [
      [
        "const run_pipeline = async (source)",
        "await load_steps(source)",
        "steps.map(async (step)",
        "await step.run(source)",
      ],
      [
        "async (source)",
        "await load_steps(source)",
        ".map(async (step)",
        "await step.run(source)",
      ],
      [
        "async function(source)",
        "await load_steps(source)",
        ".map((step) => async function",
        "await step.run(source)",
      ],
      [
        "async function run_pipeline(source)",
        "await load_steps(source)",
        ".map((step) => async function",
        "await step.run(source)",
      ],
      [
        "async function run_pipeline(source)",
        "await load_steps(source)",
        ".map(async (step)=>await step.run(source)",
      ],
    ],
  },
  {
    name: "async-iife",
    source: "(async function() {\n  await setup();\n  await run();\n})();\n",
    expected: ["async", "await setup()", "await run()"],
    expectedAny: [
      ["(async function()", "await setup()", "await run()"],
      ["async function()", "await setup()", "await run()"],
      ["async", "await setup()", "await run()"],
    ],
  },
  {
    name: "async-arrow-object-rest",
    source:
      "const load_user = async (config) => {\n  const source = config == null ? await load_config() : config;\n  const { id, token, ...options } = source;\n  const session = await open_session(token);\n  return await fetch_user(id, { ...options, session });\n};\nuse(load_user);\n",
    expected: [
      "const load_user = async (config)",
      "{ id, token, ...options }",
      "const source = config == null ? await load_config() : config",
      "await load_config()",
      "await open_session(token)",
      "return await fetch_user(id, {",
      "...options",
    ],
    expectedAny: [
      [
        "const load_user = async (config)",
        "{ id, token, ...options }",
        "const source = config == null ? await load_config() : config",
        "await load_config()",
        "await open_session(token)",
        "return await fetch_user(id, {",
        "...options",
      ],
      [
        "const load_user = async (config)",
        "{ id, token, ...options }",
        "const source = config ?? await load_config()",
        "await load_config()",
        "await open_session(token)",
        "return await fetch_user(id, {",
        "...options",
      ],
      [
        "async (config)",
        "config ?? await load_config()",
        "{ id, token, ...options }",
        "await open_session(token)",
        "return await fetch_user(id, {",
        "...options",
      ],
      [
        "const load_user = async (config)",
        "{ id, token, ...options } = source",
        "await load_config()",
        "await open_session(token)",
        "return await fetch_user(id, {",
        "...options",
      ],
      [
        "async function",
        "await load_config()",
        "await open_session(",
        "return await fetch_user(",
        "...",
      ],
      [
        "use(async",
        "await load_config()",
        "await open_session(",
        "return await fetch_user(",
        "...",
      ],
    ],
  },
  {
    name: "class-async-method",
    source:
      "class Client {\n  async fetchInternal(request, init) {\n    return await send(request, init);\n  }\n}\nuse(Client);\n",
    expected: ["fetchInternal", "async", "await send"],
  },
  {
    name: "generator-basic",
    source: "function* read_items(items) {\n  yield first_item(items);\n  yield second_item(items);\n}\n",
    expected: ["function* read_items(items)", "yield first_item(items)", "yield second_item(items)"],
  },
  {
    name: "generator-simple-loop",
    source:
      "function* iter_items(items) {\n  for (let index = 0; index < items.length; index++) {\n    yield items[index];\n  }\n}\n",
    expected: [
      "function* iter_items(items)",
      "for",
      "yield items[index]",
    ],
    expectedAny: [
      ["function* iter_items(items)", "for (let index = 0", "yield items[index]"],
      ["function* iter_items(items)", "for(;", "yield items["],
      ["function*", "for", "yield items["],
    ],
  },
  {
    name: "generator-try-catch",
    source:
      "function* fetch_items(source) {\n  try {\n    yield start_fetch(source);\n    yield finish_fetch(source);\n  } catch (error) {\n    handle(error);\n  }\n}\n",
    expected: [
      "function* fetch_items(source)",
      "try",
      "yield start_fetch(source)",
      "yield finish_fetch(source)",
      "catch",
      "handle(error)",
    ],
  },
  {
    name: "generator-try-finally",
    source:
      "function* process_stream(stream) {\n  try {\n    yield open_stream(stream);\n    yield read_stream(stream);\n  } finally {\n    close_stream(stream);\n  }\n}\n",
    expected: [
      "function* process_stream(stream)",
      "try",
      "yield open_stream(stream)",
      "yield read_stream(stream)",
      "finally",
      "close_stream(stream)",
    ],
    rejected: [".finish(", ".f("],
  },
  {
    name: "generator-try-finally-delegate",
    source:
      "function* read_all(source) {\n  try {\n    yield start_read(source);\n    yield* read_chunks(source);\n    return yield finish_read(source);\n  } finally {\n    yield close_reader(source);\n  }\n}\n",
    expected: [
      "function* read_all(source)",
      "try",
      "yield start_read(source)",
      "yield* read_chunks(source)",
      "return yield finish_read(source)",
      "finally",
      "yield close_reader(source)",
    ],
    expectedAny: [
      [
        "function* read_all(source)",
        "try",
        "yield start_read(source)",
        "yield* read_chunks(source)",
        "return yield finish_read(source)",
        "finally",
        "yield close_reader(source)",
      ],
      [
        "function*",
        "try",
        "yield start_read(",
        "yield* read_chunks(",
        "return yield finish_read(",
        "finally",
        "yield close_reader(",
      ],
    ],
  },
];

const RESERVED_WORDS = new Set([
  "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "do",
  "else", "export", "extends", "finally", "for", "function", "if", "import", "in", "let", "new",
  "of", "return", "switch", "throw", "try", "var", "while", "yield",
]);

function validateRecovered({ snippet, shape, recovered }) {
  if (!shape.tools.some((tool) => tool.includes("mangle"))) {
    return undefined;
  }

  const expectedGroups = expectedNeedleGroups(snippet);
  const sourceMapping = collectLocalNameMapping(snippet.source);
  const recoveredMapping = collectLocalNameMapping(recovered);
  const normalizedRecovered = normalizeCode(recovered, recoveredMapping);
  const missingGroups = expectedGroups.map((group) =>
    group
      .map((needle) => normalizeCode(needle, sourceMapping))
      .filter((needle) => !normalizedRecovered.includes(needle)),
  );

  if (missingGroups.some((group) => group.length === 0)) {
    return { recovered: true, notes: "mangle-equivalent syntax present" };
  }

  return undefined;
}

function expectedNeedleGroups(snippet) {
  if (snippet.expectedAny) {
    return snippet.expectedAny.map((group) => (Array.isArray(group) ? group : [group]));
  }
  return [Array.isArray(snippet.expected) ? snippet.expected : [snippet.expected]];
}

function collectLocalNameMapping(code) {
  const names = [];
  const add = (name) => {
    if (name && !RESERVED_WORDS.has(name) && !names.includes(name)) {
      names.push(name);
    }
  };

  for (const match of code.matchAll(/\b(?:async\s+)?function\*?\s*([A-Za-z_$][\w$]*)?\s*\(([^)]*)\)/g)) {
    add(match[1]);
    collectBindingNames(match[2]).forEach(add);
  }

  for (const match of code.matchAll(/\b(?:const|let|var)\s+([^;\n]+)/g)) {
    collectDeclaratorNames(match[1]).forEach(add);
  }

  for (const match of code.matchAll(/\bcatch\s*\(([^)]*)\)/g)) {
    collectBindingNames(match[1]).forEach(add);
  }

  for (const match of code.matchAll(/\(([^)]*)\)\s*=>/g)) {
    collectBindingNames(match[1]).forEach(add);
  }

  for (const match of code.matchAll(/([A-Za-z_$][\w$]*)\s*=>/g)) {
    add(match[1]);
  }

  const mapping = new Map();
  names.forEach((name) => mapping.set(name, "_"));
  return mapping;
}

function collectDeclaratorNames(text) {
  const names = [];
  let current = "";
  let depth = 0;
  for (const ch of text) {
    if (ch === "{" || ch === "[" || ch === "(") depth++;
    if (ch === "}" || ch === "]" || ch === ")") depth = Math.max(0, depth - 1);
    if (ch === "," && depth === 0) {
      names.push(...collectDeclaratorBindingNames(current));
      current = "";
    } else {
      current += ch;
    }
  }
  names.push(...collectDeclaratorBindingNames(current));
  return names;
}

function collectDeclaratorBindingNames(declarator) {
  // Split at the declarator's init `=` (depth 0), not a nested destructuring
  // default like `{ name } = {}` / `[a] = []` (depth > 0). Splitting at the
  // first `=` would drop every binding after an earlier default from the
  // rename map, breaking name normalization for mangled output.
  const eq = topLevelAssignIndex(declarator);
  const left = eq === -1 ? declarator : declarator.slice(0, eq);
  return collectBindingNames(left);
}

function topLevelAssignIndex(text) {
  let depth = 0;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (ch === "{" || ch === "[" || ch === "(") {
      depth++;
    } else if (ch === "}" || ch === "]" || ch === ")") {
      depth = Math.max(0, depth - 1);
    } else if (ch === "=" && depth === 0) {
      const prev = text[i - 1];
      const next = text[i + 1];
      // Skip ==, ===, =>, !=, <=, >= so only a real assignment `=` matches.
      if (next === "=" || next === ">" || prev === "=" || prev === "!" || prev === "<" || prev === ">") {
        continue;
      }
      return i;
    }
  }
  return -1;
}

function collectBindingNames(pattern) {
  const trimmed = pattern.trim();
  if (trimmed.startsWith("{")) {
    return collectObjectPatternBindings(trimmed);
  }
  if (trimmed.startsWith("[")) {
    return collectArrayPatternBindings(trimmed);
  }
  return [...pattern.matchAll(/[A-Za-z_$][\w$]*/g)]
    .map((match) => match[0])
    .filter((name) => !RESERVED_WORDS.has(name));
}

function collectObjectPatternBindings(pattern) {
  const inner = pattern.slice(1, pattern.length - findMatchingClose(pattern, 0));
  const names = [];
  for (const prop of splitAtTopLevel(inner, ",")) {
    const colonIdx = topLevelIndexOf(prop, ":");
    if (colonIdx !== -1) {
      // key: value — only value is a binding
      const value = prop.slice(colonIdx + 1).trim();
      names.push(...collectBindingNames(stripDefault(value)));
    } else {
      // shorthand { name } or rest ...name
      const clean = prop.replace(/^\.\.\./, "").trim();
      const stripped = stripDefault(clean);
      names.push(...collectSimpleBindingNames(stripped));
    }
  }
  return names;
}

function collectArrayPatternBindings(pattern) {
  const inner = pattern.slice(1, pattern.length - findMatchingClose(pattern, 0));
  const names = [];
  for (const elem of splitAtTopLevel(inner, ",")) {
    const clean = elem.replace(/^\.\.\./, "").trim();
    if (!clean) continue;
    names.push(...collectBindingNames(stripDefault(clean)));
  }
  return names;
}

function collectSimpleBindingNames(text) {
  return [...text.matchAll(/[A-Za-z_$][\w$]*/g)]
    .map((m) => m[0])
    .filter((n) => !RESERVED_WORDS.has(n));
}

function stripDefault(text) {
  const eq = topLevelAssignIndex(text);
  return eq === -1 ? text : text.slice(0, eq).trim();
}

function findMatchingClose(text, openIdx) {
  let depth = 0;
  const open = text[openIdx];
  const close = open === "{" ? "}" : open === "[" ? "]" : ")";
  for (let i = openIdx; i < text.length; i++) {
    if (text[i] === open) depth++;
    else if (text[i] === close) { depth--; if (depth === 0) return text.length - i; }
  }
  return 1;
}

function splitAtTopLevel(text, sep) {
  const parts = [];
  let current = "";
  let depth = 0;
  for (const ch of text) {
    if (ch === "{" || ch === "[" || ch === "(") depth++;
    if (ch === "}" || ch === "]" || ch === ")") depth = Math.max(0, depth - 1);
    if (ch === sep && depth === 0) {
      parts.push(current);
      current = "";
    } else {
      current += ch;
    }
  }
  parts.push(current);
  return parts;
}

function topLevelIndexOf(text, ch) {
  let depth = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === "{" || c === "[" || c === "(") depth++;
    if (c === "}" || c === "]" || c === ")") depth = Math.max(0, depth - 1);
    if (c === ch && depth === 0) return i;
  }
  return -1;
}

function normalizeIdentifiers(code, mapping) {
  return code.replace(/\b[A-Za-z_$][\w$]*\b/g, (name, offset) => {
    if (isPropertyName(code, offset)) {
      return name;
    }
    return mapping.get(name) ?? name;
  });
}

function isPropertyName(code, offset) {
  let index = offset - 1;
  while (index >= 0 && /\s/.test(code[index])) index--;
  if (code[index] === "." && code[index - 1] === "." && code[index - 2] === ".") {
    return false;
  }
  if (code[index] === ".") return true;
  // Check if this is an object/destructuring key: `ident:` (but not `ident::`)
  const match = code.slice(offset).match(/^[A-Za-z_$][\w$]*/);
  if (match) {
    let after = offset + match[0].length;
    while (after < code.length && /\s/.test(code[after])) after++;
    if (code[after] === ":" && code[after + 1] !== ":") return true;
  }
  return false;
}

function normalizeCode(code, mapping) {
  return normalizeIdentifiers(code, mapping).replace(/\s+/g, "");
}

const babelProfiles = [
  {
    name: "babel-7.8",
    core: "7.8.7",
    asyncPlugin: ["@babel/plugin-transform-async-to-generator", "7.8.3"],
    regeneratorPlugin: ["@babel/plugin-transform-regenerator", "7.8.7"],
  },
  {
    name: "babel-7.13",
    core: "7.13.16",
    asyncPlugin: ["@babel/plugin-transform-async-to-generator", "7.13.0"],
    regeneratorPlugin: ["@babel/plugin-transform-regenerator", "7.13.15"],
  },
  {
    name: "babel-7.28",
    core: "7.28.5",
    asyncPlugin: ["@babel/plugin-transform-async-to-generator", "7.28.6"],
    regeneratorPlugin: ["@babel/plugin-transform-regenerator", "7.28.4"],
  },
  {
    name: "babel-8-rc",
    core: "8.0.0-rc.5",
    asyncPlugin: ["@babel/plugin-transform-async-to-generator", "8.0.0-rc.5"],
    regeneratorPlugin: ["@babel/plugin-transform-regenerator", "8.0.0-rc.5"],
  },
];

const allSources = snippets.map((s) => s.source);

function babelAsyncBatch(sources, profile, mode) {
  const plugins = [profile.asyncPlugin];
  if (mode === "regenerator") plugins.push(profile.regeneratorPlugin);
  return babelMultiPluginBatch(sources, profile, plugins);
}

const transformers = [
  ...babelProfiles.flatMap((profile) =>
    ["async-generator", "regenerator"].flatMap((mode) =>
      withTerserVariants(
        `${profile.name}-${mode}`,
        allSources,
        batchRunner(() => babelAsyncBatch(allSources, profile, mode)),
      ),
    ),
  ),
  ...withTerserVariants(
    "babel-7.29-preset-env-ie11",
    allSources,
    batchRunner(() => babelPresetEnvBatch(allSources)),
  ),
  ...withTerserVariants("tsc-es5", allSources, batchRunner(() => tscBatch(allSources))),
  ...withTerserVariants("swc-es5", allSources, batchRunner(() => swcBatch(allSources))),
  ...withTerserVariants("esbuild-es2015", allSources, batchRunner(() => esbuildBatch(allSources))),
  ...withTerserVariants("source", allSources, (source) => source, { includeRaw: false }),
];

runMatrix({
  name: "async-await",
  snippets,
  transformers,
  validateRecovered,
});
