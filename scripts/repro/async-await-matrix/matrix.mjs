#!/usr/bin/env node

import {
  runMatrix, batchRunner, babelMultiPluginBatch, babelPresetEnvBatch,
  withTerserVariants, standardLowerers,
} from "../lib/runner.mjs";
import { mangleValidator } from "../lib/compare.mjs";

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
    // Clean mangle recoveries: single-use temp inlined as a trailing
    // `return await`, or the hoisted temps left split when merging across
    // awaits would hide a real regenerator-state leak.
    acceptForms: [
      "async function load_user(app_id) {\n  const response = await fetch_user(app_id);\n  return await response.json();\n}\n",
      "async function load_user(app_id) {\n  let response = await fetch_user(app_id);\n  let data = await response.json();\n  return data;\n}\n",
      "async function load_user(app_id) {\n  let response;\n  let data;\n  response = await fetch_user(app_id);\n  data = await response.json();\n  return data;\n}\n",
    ],
    expected: ["async function load_user(app_id)", "await fetch_user(app_id)", "await response.json()", "return data"],
    expectedAny: [
      ["async function load_user(app_id)", "await fetch_user(app_id)", "await response.json()", "return data"],
      ["async function load_user(app_id)", "await fetch_user(app_id)", "return await response.json()"],
      ["async function", "await fetch_user(", "return await", ".json()"],
    ],
  },
  {
    name: "async-guarded-await-join",
    source:
      "async function init() {\n  let id;\n  if (!(id = cache.get())) {\n    id = await storage.getItemAsync(KEY) ?? make();\n  }\n  storage.setItemAsync(KEY, id);\n  return id;\n}\n",
    expected: [
      "async function init()",
      "if (!(id = cache.get()))",
      "await storage.getItemAsync(KEY)",
      "storage.setItemAsync(KEY, id)",
      "return id",
    ],
    // This row targets TypeScript's forward jump into a mid-machine join;
    // other async lowerers use unrelated state-machine/helper shapes.
    transformerFilter: ({ name }) => name.startsWith("tsc-es5"),
  },
  {
    name: "async-arrow-captured-this",
    source:
      "class Queue {\n  initialize(items) {\n    return items.map(async (item) => {\n      await item.load();\n      this.queue.push(item);\n      return this.errorBoundary;\n    });\n  }\n}\nuse(Queue);\n",
    // TypeScript ES5 captures the method receiver in an alias passed as
    // __awaiter's thisArg. Keeping that alias after recovery is semantically
    // equivalent to the original arrow's lexical `this`.
    acceptForms: [
      "class Queue {\n  initialize(items) {\n    const self = this;\n    return items.map(async (item) => {\n      await item.load();\n      self.queue.push(item);\n      return self.errorBoundary;\n    });\n  }\n}\nuse(Queue);\n",
    ],
    expected: [
      "class Queue",
      "initialize(items)",
      "items.map(async (item)",
      "await item.load()",
      ".queue.push(item)",
      ".errorBoundary",
    ],
    // TypeScript is the producer that passes a captured receiver alias as
    // __awaiter's thisArg while its generator body still refers to `this`.
    transformerFilter: ({ name }) => name.startsWith("tsc-es5"),
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
    acceptForms: [
      // lock merged into its first assignment; payload stays a bare `let`
      // because it is assigned inside the try (a different statement list).
      "async function save_record(record) {\n  let payload;\n  let lock = await acquire_lock(record.id);\n  try {\n    payload = await prepare_record(record);\n    return await commit_record(payload);\n  } finally {\n    await lock.release();\n  }\n}\n",
      // Fully split temp declarations are also valid after the conservative
      // async hardening keeps await assignments out of declaration initializers.
      "async function save_record(record) {\n  let lock;\n  let payload;\n  lock = await acquire_lock(record.id);\n  try {\n    payload = await prepare_record(record);\n    return await commit_record(payload);\n  } finally {\n    await lock.release();\n  }\n}\n",
    ],
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
    // Mangle shapes are compared structurally (names ignored). Some toolchains
    // recover the C-style loop as an idiomatic `for…of`, with the enable check
    // as a `continue` guard or an `if` wrapper. Both are clean recoveries.
    acceptForms: [
      "async function collect_enabled(items) {\n  const output = [];\n  for (const item of items) {\n    if (!item.enabled) { continue; }\n    try { output.push(await fetch_item(item.id)); }\n    catch (error) { output.push(await recover_item(item, error)); }\n  }\n  return output;\n}\n",
      "async function collect_enabled(items) {\n  const output = [];\n  for (const item of items) {\n    if (item.enabled) {\n      try { output.push(await fetch_item(item.id)); }\n      catch (error) { output.push(await recover_item(item, error)); }\n    }\n  }\n  return output;\n}\n",
      // C-style loop preserved; hoisted output/index merged into their inits,
      // the item temp folded into the loop guard.
      "async function collect_enabled(items) {\n  let item;\n  let output = [];\n  let index = 0;\n  for (; index < items.length; index++) {\n    if (!(item = items[index]).enabled) continue;\n    try { output.push(await fetch_item(item.id)); }\n    catch (error) { output.push(await recover_item(item, error)); }\n  }\n  return output;\n}\n",
      // Same C-style recovery when the index initializer remains split.
      "async function collect_enabled(items) {\n  let index;\n  let item;\n  let output = [];\n  index = 0;\n  for (; index < items.length; index++) {\n    if (!(item = items[index]).enabled) continue;\n    try { output.push(await fetch_item(item.id)); }\n    catch (error) { output.push(await recover_item(item, error)); }\n  }\n  return output;\n}\n",
      "async function collect_enabled(items) {\n  let index;\n  let item;\n  let output = [];\n  index = 0;\n  for (; index < items.length; index++) {\n    if (!(item = items[index]).enabled) { continue; }\n    try { output.push(await fetch_item(item.id)); }\n    catch (error) { output.push(await recover_item(item, error)); }\n  }\n  return output;\n}\n",
    ],
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
    // Clean mangle recovery: `== null ? :` folds to `??` and the single-use
    // temps are inlined into the returned object.
    acceptForms: [
      "async function normalize_user(input) {\n  const source = input ?? await load_user();\n  const { id, profile: { name } = {}, tags: [primary, , backup] = [] } = source;\n  return {\n    id,\n    name,\n    primary,\n    backup: backup ?? await load_backup(id),\n    meta: await load_meta(id)\n  };\n}\n",
      "async function normalize_user(input) {\n  let source;\n  let id;\n  let name;\n  let primary;\n  let backup;\n  let resolved_backup;\n  let meta;\n  let resolved;\n  resolved = input ?? await load_user();\n  ({ id, profile: { name } = {}, tags: [primary, , backup] = [] } = source = resolved);\n  resolved_backup = backup ?? await load_backup(id);\n  meta = await load_meta(id);\n  return { id, name, primary, backup: resolved_backup, meta };\n}\n",
      "async function normalize_user(input) {\n  let source;\n  let resolved;\n  let id;\n  let name;\n  let primary;\n  let backup;\n  let resolved_backup;\n  let meta;\n  resolved = input ?? await load_user();\n  ({ id, profile: { name } = {}, tags: [primary, , backup] = [] } = source = resolved);\n  resolved_backup = backup ?? await load_backup(id);\n  meta = await load_meta(id);\n  return { id, name, primary, backup: resolved_backup, meta };\n}\n",
    ],
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
    // Clean mangle recovery: the single-use arrow is inlined into `use(...)` as
    // an async function expression.
    acceptForms: [
      "use(async function load_user(app_id) {\n  return await fetch_user(app_id);\n});\n",
      "use(async (app_id) => await fetch_user(app_id));\n",
      "const ignored = undefined;\nuse(async (app_id) => await fetch_user(app_id));\n",
    ],
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
    // Clean mangle recovery: single-use `steps` temp inlined into the chained
    // call.
    acceptForms: [
      "const run_pipeline = async (source) => {\n  return (await load_steps(source)).map(async (step) => await step.run(source));\n};\nuse(run_pipeline);\n",
      // steps temp merged into its first assignment.
      "const run_pipeline = async (source) => {\n  let steps = await load_steps(source);\n  return steps.map(async (step) => await step.run(source));\n};\nuse(run_pipeline);\n",
      "const run_pipeline = async (source) => {\n  let steps;\n  steps = await load_steps(source);\n  return steps.map(async (step) => await step.run(source));\n};\nuse(run_pipeline);\n",
      // The single-use async arrow may be inlined into its call site.
      "use(async (source) => {\n  let steps;\n  steps = await load_steps(source);\n  return steps.map(async (step) => await step.run(source));\n});\n",
      "use(async (source) => {\n  let steps;\n  return (steps = await load_steps(source)).map(async (step) => await step.run(source));\n});\n",
      "use(async function run_pipeline(source) {\n  let steps;\n  return (steps = await load_steps(source)).map(async (step) => await step.run(source));\n});\n",
      "use(async (source) => {\n  return (await load_steps(source)).map(async (step) => await step.run(source));\n});\n",
      "const ignored = undefined;\nuse(async (source) => {\n  return (await load_steps(source)).map(async (step) => await step.run(source));\n});\n",
    ],
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
    name: "async-for-await-break-finally",
    source:
      "async function consume_stream(stream) {\n  const output = [];\n  try {\n    for await (const item of stream) {\n      if (item.done) break;\n      output.push(await normalize_item(item));\n    }\n  } finally {\n    await close_stream(stream);\n  }\n  return output;\n}\n",
    expected: [
      "async function consume_stream(stream)",
      "for await (const item of stream)",
      "break",
      "await normalize_item(item)",
      "finally",
      "await close_stream(stream)",
      "return output",
    ],
  },
  {
    name: "async-arrow-object-rest",
    source:
      "const load_user = async (config) => {\n  const source = config == null ? await load_config() : config;\n  const { id, token, ...options } = source;\n  const session = await open_session(token);\n  return await fetch_user(id, { ...options, session });\n};\nuse(load_user);\n",
    // Clean mangle recovery: `== null ? :` folds to `??`.
    acceptForms: [
      "const load_user = async (config) => {\n  const source = config ?? await load_config();\n  const { id, token, ...options } = source;\n  const session = await open_session(token);\n  return await fetch_user(id, { ...options, session });\n};\nuse(load_user);\n",
      // Mangle recovery can preserve split temps and assignment-form
      // destructuring after regenerator recovery exposes the helper callsite.
      "const load_user = async (config) => {\n  let source;\n  let id;\n  let token;\n  let options;\n  let session;\n  let resolved;\n  resolved = config ?? await load_config();\n  source = resolved;\n  ({ id, token, ...options } = source);\n  session = await open_session(token);\n  return await fetch_user(id, { ...options, session });\n};\nuse(load_user);\n",
      "const load_user = async (config) => {\n  let source;\n  let id;\n  let token;\n  let options;\n  let session;\n  source = config ?? await load_config();\n  ({ id, token, ...options } = source);\n  session = await open_session(token);\n  return await fetch_user(id, { ...options, session });\n};\nuse(load_user);\n",
    ],
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
    // Mangle recovery: C-style loop with the index hoisted and merged into its
    // init (`let index = 0; for (; …)`).
    acceptForms: [
      "function* iter_items(items) {\n  let index = 0;\n  for (; index < items.length; index++) {\n    yield items[index];\n  }\n}\n",
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

// Mangle shapes rename every local binding, so substring needles (which carry
// the original names) can't match. Instead, compare the recovered output to the
// original snippet structurally: wakaru's `debug normalize --rename` collapses
// binding names and formatting, so an alpha-equivalent recovery normalizes to
// identical source. `acceptForms` lists any *genuinely distinct* structural
// shapes wakaru may legitimately emit (e.g. for-loop vs for-of) as full
// programs — far fewer than the old per-name `expectedAny` groups, since
// renaming already absorbs the name/whitespace variants.
const { validateRecovered: mangleValidate, prewarm: prewarmComparison } = mangleValidator();

function validateRecovered(ctx) {
  const opcode = leakedStateOpcodeReturn(ctx.recovered);
  if (opcode) {
    return {
      recovered: false,
      notes: `leaked generator state opcode ${opcode}`,
      leaked: [opcode],
      missing: [],
    };
  }
  return mangleValidate(ctx);
}

function leakedStateOpcodeReturn(code) {
  const match = code.match(/\breturn\s+(?:[^;\n]*,\s*)?\[\s*([34])\s*,/s);
  return match ? `return [${match[1]}, ...]` : null;
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
  ...standardLowerers(allSources),
];

runMatrix({
  name: "async-await",
  snippets,
  transformers,
  validateRecovered,
  prewarm: prewarmComparison,
});
