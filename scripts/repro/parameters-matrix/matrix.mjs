#!/usr/bin/env node

import { runNodeBatchSync } from "../lib/tool-process.mjs";

import {
  runMatrix, batchRunner, withTerserVariants,
  ensureNodeTool, standardLowerers,
} from "../lib/runner.mjs";
import { mangleValidator } from "../lib/compare.mjs";

const snippets = [
  {
    name: "default-basic",
    source: `
function greet(name = "world", count = 1) {
  return use(name, count);
}
`,
    expected: ['function greet(name = "world", count = 1)', "return use(name, count)"],
  },
  {
    name: "default-before-required",
    source: `
function range(start = 0, end) {
  return use(start, end);
}
`,
    expected: ["function range(start = 0, end)", "return use(start, end)"],
  },
  {
    name: "object-destructured-default",
    source: `
function pick({ name, age = 0 } = {}) {
  return use(name, age);
}
`,
    expected: ["function pick({ name, age = 0 } = {})", "return use(name, age)"],
  },
  {
    name: "object-alias-default",
    source: `
function config({ mode: appMode = "prod" } = {}) {
  return use(appMode);
}
`,
    expected: ['function config({ mode: appMode = "prod" } = {})', "return use(appMode)"],
  },
  {
    name: "array-destructured-default",
    source: `
function first([head, second = fallback] = []) {
  return use(head, second);
}
`,
    expected: ["function first([head, second = fallback] = [])", "return use(head, second)"],
  },
  {
    name: "nested-default",
    source: `
function nested({ outer: { value = fallbackValue } = {} } = {}) {
  return use(value);
}
`,
    expected: ["function nested({ outer: { value = fallbackValue } = {} } = {})", "return use(value)"],
  },
  {
    name: "computed-destructured-default",
    source: `
function pick(property_key, { [property_key]: value = fallback } = {}) {
  return use(value);
}
`,
    expected: [
      "function pick(property_key, { [property_key]: value = fallback } = {})",
      "return use(value)",
    ],
  },
];

const babelProfiles = [
  {
    name: "babel-7.8",
    core: "7.8.7",
    destructuringPlugin: ["@babel/plugin-transform-destructuring", "7.8.3"],
    parametersPlugin: ["@babel/plugin-transform-parameters", "7.8.7"],
    modes: ["spec", "loose"],
  },
  {
    name: "babel-7.13",
    core: "7.13.16",
    destructuringPlugin: ["@babel/plugin-transform-destructuring", "7.13.17"],
    parametersPlugin: ["@babel/plugin-transform-parameters", "7.13.0"],
    modes: ["spec", "loose", "iterableIsArray"],
  },
  {
    name: "babel-7.28",
    core: "7.28.5",
    destructuringPlugin: ["@babel/plugin-transform-destructuring", "7.28.5"],
    parametersPlugin: ["@babel/plugin-transform-parameters", "7.27.7"],
    modes: ["spec", "loose", "iterableIsArray"],
  },
  {
    name: "babel-8",
    core: "8.0.6",
    destructuringPlugin: ["@babel/plugin-transform-destructuring", "8.0.5"],
    parametersPlugin: ["@babel/plugin-transform-parameters", "8.0.5"],
    modes: ["spec", "loose", "iterableIsArray"],
  },
];

function babelModeOptions(mode) {
  switch (mode) {
    case "spec":
      return {};
    case "loose":
      return { loose: true };
    case "iterableIsArray":
      return { assumptions: { iterableIsArray: true } };
    default:
      throw new Error(`unsupported Babel mode ${mode}`);
  }
}

function babelParametersBatch(sources, profile, options) {
  const [destructuringName, destructuringVersion] = profile.destructuringPlugin;
  const [parametersName, parametersVersion] = profile.parametersPlugin;
  const packages = [
    `@babel/core@${profile.core}`,
    `${destructuringName}@${destructuringVersion}`,
    `${parametersName}@${parametersVersion}`,
  ];
  const toolKey = `babel-${profile.core}-destructuring-parameters`;
  const toolDir = ensureNodeTool(toolKey, packages);
  const pluginOptions = { ...options };
  delete pluginOptions.assumptions;
  const helperSource = `
import fs from "node:fs";
const babelModule = await import("@babel/core");
const destructuringModule = await import(${JSON.stringify(destructuringName)});
const parametersModule = await import(${JSON.stringify(parametersName)});
const babel = babelModule.default ?? babelModule;
const destructuring = destructuringModule.default ?? destructuringModule;
const parameters = parametersModule.default ?? parametersModule;
const options = JSON.parse(process.env.MATRIX_BABEL_OPTIONS || "{}");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    const config = {
      filename: "input.js", babelrc: false, configFile: false, comments: false, compact: false,
      plugins: [
        [parameters, options.pluginOptions || {}],
        [destructuring, options.pluginOptions || {}],
      ],
    };
    if (options.assumptions) {
      config.assumptions = options.assumptions;
    }
    return { code: babel.transformSync(source, config).code };
  } catch (e) { return { error: e.message }; }
});
process.stdout.write(JSON.stringify(results));
`;
  return runNodeBatchSync(helperSource, sources, {
    label: "babel-parameters-batch.mjs",
    cwd: toolDir,
    env: { MATRIX_BABEL_OPTIONS: JSON.stringify({ ...options, pluginOptions }) },
  });
}

const allSources = snippets.map((s) => s.source);

const transformers = [
  ...babelProfiles.flatMap((profile) =>
    profile.modes.flatMap((mode) =>
      withTerserVariants(
        `${profile.name}-${mode}`,
        allSources,
        batchRunner(() => babelParametersBatch(allSources, profile, babelModeOptions(mode))),
      ),
    ),
  ),
  ...standardLowerers(allSources),
];

runMatrix({
  name: "parameters",
  snippets,
  transformers,
  ...mangleValidator(),
});
