#!/usr/bin/env node

import {
  runMatrix, batchRunner, withTerserVariants, ensureNodeTool, standardLowerers,
} from "../lib/runner.mjs";
import { mangleValidator } from "../lib/compare.mjs";
import { join } from "node:path";
import { writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const snippets = [
  {
    name: "spread-basic",
    source: "const out = { ...app_info, name: value, ...base_info };\nuse(out);\n",
    expected: ["...app_info", "name: value", "...base_info"],
    execute: { env: { app_info: { name: "app", size: 2 }, base_info: { name: "base", tag: 3 }, value: "mid" } },
  },
  {
    name: "spread-leading-property",
    source: "const out = { id: app_id, ...app_info };\nuse(out);\n",
    expected: ["id: app_id", "...app_info"],
    execute: { env: { app_id: 7, app_info: { id: 1, size: 2 } } },
  },
  {
    name: "spread-nullish-source",
    source: "const out = { ...app_info };\nuse(out);\n",
    expected: ["...app_info"],
    execute: { env: { app_info: { size: 2 } } },
  },
  {
    name: "rest-basic",
    source: "const { name, ...rest_info } = app_info;\nuse(name, rest_info);\n",
    acceptForms: [
      "const o = app_info;\nconst { name, ...rest_info } = o;\nuse(name, rest_info);\n",
    ],
    expected: ["const {", "name", "...rest_info"],
    execute: { env: { app_info: { name: "app", size: 2, tag: 3 } } },
  },
  {
    name: "rest-rename-default",
    source:
      "const { name: app_name, version = fallback_version, ...rest_info } = app_info;\nuse(app_name, version, rest_info);\n",
    acceptForms: [
      "const o = app_info;\nconst { name: app_name, version = fallback_version, ...rest_info } = o;\nuse(app_name, version, rest_info);\n",
    ],
    expected: ["name: app_name", "version = fallback_version", "...rest_info"],
    execute: { env: { app_info: { name: "app", size: 2 }, fallback_version: 9 } },
  },
  {
    name: "rest-string-key",
    source:
      'const { "app-id": app_id, name, ...rest_info } = app_info;\nuse(app_id, name, rest_info);\n',
    acceptForms: [
      'const o = app_info;\nconst { "app-id": app_id, name, ...rest_info } = o;\nuse(app_id, name, rest_info);\n',
    ],
    expected: ['"app-id": app_id', "name", "...rest_info"],
    execute: { env: { app_info: { "app-id": 7, name: "app", size: 2 } } },
  },
  {
    name: "rest-computed-key",
    source:
      "const property_key = get_key();\nconst { [property_key]: picked, ...rest_info } = app_info;\nuse(picked, rest_info);\n",
    acceptForms: [
      "const property_key = get_key();\nconst o = app_info;\nconst { [property_key]: picked, ...rest_info } = o;\nuse(picked, rest_info);\n",
    ],
    expected: ["[property_key]: picked", "...rest_info", "use(picked, rest_info)"],
    execute: {
      env: { app_info: { size: 2, name: "app" } },
      returns: { get_key: "size" },
    },
  },
  {
    name: "spread-rest-combined",
    source:
      "const { name, ...rest_info } = app_info;\nconst out = { ...rest_info, name };\nuse(out);\n",
    acceptForms: [
      "const o = app_info;\nconst { name, ...rest_info } = o;\nconst out = { ...rest_info, name };\nuse(out);\n",
    ],
    expected: ["...rest_info", "name"],
    execute: { env: { app_info: { name: "app", size: 2 } } },
  },
  {
    name: "rest-mutated-binding",
    // Pins the declaration-kind contract end to end: the recovered
    // destructuring must stay writable when the rest binding is reassigned.
    source:
      "let { name, ...rest_info } = app_info;\nrest_info = mutate(rest_info);\nuse(name, rest_info);\n",
    // A generated alias of the env-injected global is deliberately outside
    // SmartInline's frozen-local proof. Accept that single-read producer shape
    // without teaching the rule that unresolved globals are stable.
    acceptForms: [
      "let o = app_info;\nlet { name, ...rest_info } = o;\nrest_info = mutate(rest_info);\nuse(name, rest_info);\n",
    ],
    expected: ["...rest_info", "rest_info = mutate(rest_info)"],
    execute: { env: { app_info: { name: "app", size: 2 } } },
  },
];

const babelProfiles = [
  {
    name: "babel-7.8",
    core: "7.8.7",
    plugin: ["@babel/plugin-proposal-object-rest-spread", "7.8.3"],
    modes: ["spec", "loose", "useBuiltIns"],
  },
  {
    name: "babel-7.13",
    core: "7.13.16",
    plugin: ["@babel/plugin-proposal-object-rest-spread", "7.13.8"],
    modes: ["spec", "loose", "useBuiltIns"],
  },
  {
    name: "babel-7.28",
    core: "7.28.5",
    plugin: ["@babel/plugin-transform-object-rest-spread", "7.28.6"],
    modes: ["spec", "loose", "useBuiltIns"],
  },
  {
    name: "babel-8-rc",
    core: "8.0.0-rc.5",
    plugin: ["@babel/plugin-transform-object-rest-spread", "8.0.0-rc.5"],
    modes: ["spec", "loose", "useBuiltIns"],
  },
];

function babelModeOptions(mode) {
  switch (mode) {
    case "spec":
      return { pluginOptions: {} };
    case "loose":
      return { pluginOptions: { loose: true } };
    case "useBuiltIns":
      return { pluginOptions: { useBuiltIns: true } };
    default:
      throw new Error(`unsupported Babel mode ${mode}`);
  }
}

function babelObjectRestSpreadBatch(sources, profile, options) {
  const pluginName = profile.plugin[0];
  const pluginVersion = profile.plugin[1];
  const packages = [`@babel/core@${profile.core}`, `${pluginName}@${pluginVersion}`];
  const toolKey = `babel-${profile.core}-object-rest-spread`;
  const toolDir = ensureNodeTool(toolKey, packages);
  const helper = join(toolDir, "babel-object-rest-spread-batch.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
const babelModule = await import("@babel/core");
const pluginModule = await import(${JSON.stringify(pluginName)});
const babel = babelModule.default ?? babelModule;
const plugin = pluginModule.default ?? pluginModule;
const options = JSON.parse(process.env.MATRIX_BABEL_OPTIONS || "{}");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: babel.transformSync(source, {
      filename: "input.js", babelrc: false, configFile: false, comments: false, compact: false,
      plugins: [[plugin, options.pluginOptions || {}]],
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
    env: { ...process.env, MATRIX_BABEL_OPTIONS: JSON.stringify(options) },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`babel batch exited ${result.status}: ${result.stderr}`);
  const outputs = JSON.parse(result.stdout);
  const map = new Map();
  for (let i = 0; i < sources.length; i++) {
    map.set(sources[i], outputs[i].error ? new Error(outputs[i].error) : outputs[i].code);
  }
  return map;
}

const allSources = snippets.map((s) => s.source);

const transformers = [
  ...babelProfiles.flatMap((profile) =>
    profile.modes.flatMap((mode) =>
      withTerserVariants(
        `${profile.name}-${mode}`,
        allSources,
        batchRunner(() => babelObjectRestSpreadBatch(allSources, profile, babelModeOptions(mode))),
      ),
    ),
  ),
  ...standardLowerers(allSources, { esbuildTarget: "es2017", swcExternalHelpers: true }),
];

runMatrix({
  name: "object-rest-spread",
  snippets,
  transformers,
  ...mangleValidator(),
});
