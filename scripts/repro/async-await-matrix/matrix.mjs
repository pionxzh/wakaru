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
    ],
  },
  {
    name: "async-arrow",
    source: "const load_user = async (app_id) => await fetch_user(app_id);\nuse(load_user);\n",
    expected: ["async (app_id)", "await fetch_user(app_id)"],
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
});
