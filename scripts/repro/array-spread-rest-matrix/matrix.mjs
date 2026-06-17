#!/usr/bin/env node

import {
  runMatrix, batchRunner, terserBatch, withTerserVariants,
  ensureNodeTool, standardLowerers,
} from "../lib/runner.mjs";
import { matchesAnyForm, prewarmNormalize } from "../lib/compare.mjs";
import { join } from "node:path";
import { writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

// A recovered output should never still contain a helper *import path*: if it
// does, an imported helper (`@swc/helpers`, `@babel/runtime`) was not folded and
// its import not removed. This catches the "syntax recovered but helper import
// lingers" class regardless of how the local binding was renamed.
const HELPER_IMPORT_LEAKS = ["@swc/helpers/", "@babel/runtime/helpers/"];

// Inlined helper *function* names for the array/spread/rest families (the
// non-external lowering has no import path, only these bodies). Their presence
// in recovered output means the top-level helper or one of its sub-helpers was
// left behind. The two names per helper cover Babel camelCase and swc snake_case.
const ARRAY_HELPER_LEAKS = [
  "_toConsumableArray", "_to_consumable_array",
  "_toArray", "_to_array",
  "_slicedToArray", "_sliced_to_array",
  "_arrayWithoutHoles", "_array_without_holes",
  "_arrayWithHoles", "_array_with_holes",
  "_iterableToArrayLimit", "_iterable_to_array_limit",
  "_iterableToArray", "_iterable_to_array",
  "_unsupportedIterableToArray", "_unsupported_iterable_to_array",
  "_arrayLikeToArray", "_array_like_to_array",
  "_nonIterableSpread", "_non_iterable_spread",
  "_nonIterableRest", "_non_iterable_rest",
  // Babel's `arrayLikeIsIterable` assumption wraps the destructuring source in
  // this helper instead of toArray/slicedToArray; wakaru does not yet recover it.
  "_maybeArrayLike", "_maybe_array_like",
];

// Rest *parameters* lower to an `arguments` copy loop, not a helper; these names
// only survive when that loop was not recovered back to `...rest`.
const REST_PARAM_LEAKS = ["arguments", "_len", "_key"];

const ARRAY_HELPER_REJECTED = [...HELPER_IMPORT_LEAKS, ...ARRAY_HELPER_LEAKS];

const snippets = [
  {
    name: "array-spread-basic",
    source: "const out = [head, ...items, tail];\nuse(out);\n",
    expected: ["head", "...items", "tail"],
    rejected: ARRAY_HELPER_REJECTED,
  },
  {
    name: "array-spread-multiple",
    source: "const out = [...left_items, middle, ...right_items];\nuse(out);\n",
    expected: ["...left_items", "middle", "...right_items"],
    rejected: ARRAY_HELPER_REJECTED,
  },
  {
    name: "call-spread-free",
    source: "const out = build(app_id, ...items, tail);\nuse(out);\n",
    expected: ["build(app_id, ...items, tail)"],
    rejected: ARRAY_HELPER_REJECTED,
  },
  {
    name: "call-spread-method",
    source: "const out = app_info.build(prefix, ...items, tail);\nuse(out);\n",
    expected: ["app_info.build(prefix, ...items, tail)"],
    rejected: ARRAY_HELPER_REJECTED,
  },
  {
    name: "rest-param-basic",
    source: "function collect(first, ...rest_items) {\n  return use(first, rest_items);\n}\n",
    expected: ["function collect(first, ...rest_items)", "return use(first, rest_items)"],
    rejected: [...HELPER_IMPORT_LEAKS, ...REST_PARAM_LEAKS],
  },
  {
    name: "rest-param-offset",
    source:
      "function collect(app_id, version, ...rest_items) {\n  return use(app_id, version, rest_items);\n}\n",
    expected: ["function collect(app_id, version, ...rest_items)", "return use(app_id, version, rest_items)"],
    rejected: [...HELPER_IMPORT_LEAKS, ...REST_PARAM_LEAKS],
  },
  {
    name: "array-rest-basic",
    source: "const [first, ...rest_items] = items;\nuse(first, rest_items);\n",
    expected: ["const [first, ...rest_items] = items", "use(first, rest_items)"],
    rejected: ARRAY_HELPER_REJECTED,
  },
  {
    name: "array-rest-default-hole",
    source: "const [first, , second = fallback, ...rest_items] = items;\nuse(first, second, rest_items);\n",
    expected: ["first", "second = fallback", "...rest_items"],
    rejected: ARRAY_HELPER_REJECTED,
  },
  {
    name: "array-destructure-tuple",
    source:
      'import { useState } from "react";\nconst [current, setCurrent] = useState(value);\nuse(current, setCurrent);\n',
    acceptForms: [
      'import { useState } from "react";\nconst unused = undefined;\nconst [current, setCurrent] = useState(value);\nuse(current, setCurrent);\n',
    ],
    expected: ["const [current, setCurrent]", "use(current, setCurrent)"],
    rejected: [...ARRAY_HELPER_REJECTED, "_useState[0]", "_useState[1]"],
  },
  {
    name: "array-destructure-assignment",
    source:
      'import { useState } from "react";\nfunction Component() {\n  var current;\n  var setCurrent;\n  [current, setCurrent] = useState(value);\n  use(current, setCurrent);\n}\nComponent();\n',
    acceptForms: [
      'import { useState } from "react";\nfunction Component() {\n  const [current, setCurrent] = useState(value);\n  use(current, setCurrent);\n}\nComponent();\n',
      'import { useState } from "react";\nfunction Component() {\n  let current;\n  let setCurrent;\n  [current, setCurrent] = useState(value);\n  use(current, setCurrent);\n}\nComponent();\n',
    ],
    expected: ["useState(value)", "use(current, setCurrent)"],
    rejected: [...ARRAY_HELPER_REJECTED, "[0]", "[1]"],
  },
];

const babelProfiles = [
  {
    name: "babel-7.8",
    core: "7.8.7",
    spreadPlugin: ["@babel/plugin-transform-spread", "7.8.3"],
    destructuringPlugin: ["@babel/plugin-transform-destructuring", "7.8.3"],
    parametersPlugin: ["@babel/plugin-transform-parameters", "7.8.7"],
    modes: ["spec", "loose"],
  },
  {
    name: "babel-7.13",
    core: "7.13.16",
    spreadPlugin: ["@babel/plugin-transform-spread", "7.13.0"],
    destructuringPlugin: ["@babel/plugin-transform-destructuring", "7.13.17"],
    parametersPlugin: ["@babel/plugin-transform-parameters", "7.13.0"],
    modes: ["spec", "loose", "iterableIsArray", "arrayLikeIsIterable"],
  },
  {
    name: "babel-7.28",
    core: "7.28.5",
    spreadPlugin: ["@babel/plugin-transform-spread", "7.28.6"],
    destructuringPlugin: ["@babel/plugin-transform-destructuring", "7.28.5"],
    parametersPlugin: ["@babel/plugin-transform-parameters", "7.27.7"],
    modes: ["spec", "loose", "iterableIsArray", "arrayLikeIsIterable"],
  },
  {
    name: "babel-8-rc",
    core: "8.0.0-rc.5",
    spreadPlugin: ["@babel/plugin-transform-spread", "8.0.0-rc.5"],
    destructuringPlugin: ["@babel/plugin-transform-destructuring", "8.0.0-rc.5"],
    parametersPlugin: ["@babel/plugin-transform-parameters", "8.0.0-rc.5"],
    modes: ["spec", "loose", "iterableIsArray", "arrayLikeIsIterable"],
  },
];

function babelModeOptions(mode) {
  switch (mode) {
    case "spec":
      return { assumptions: {}, pluginOptions: {} };
    case "loose":
      return { assumptions: {}, pluginOptions: { loose: true } };
    case "iterableIsArray":
      return { assumptions: { iterableIsArray: true }, pluginOptions: {} };
    case "arrayLikeIsIterable":
      return { assumptions: { arrayLikeIsIterable: true }, pluginOptions: {} };
    default:
      throw new Error(`unsupported Babel mode ${mode}`);
  }
}

function babelSpreadRestBatch(sources, profile, options) {
  const [spreadName, spreadVersion] = profile.spreadPlugin;
  const [destructuringName, destructuringVersion] = profile.destructuringPlugin;
  const [parametersName, parametersVersion] = profile.parametersPlugin;
  const packages = [
    `@babel/core@${profile.core}`,
    `${spreadName}@${spreadVersion}`,
    `${destructuringName}@${destructuringVersion}`,
    `${parametersName}@${parametersVersion}`,
  ];
  const toolKey = `babel-${profile.core}-spread-destructuring-parameters`;
  const toolDir = ensureNodeTool(toolKey, packages);
  const helper = join(toolDir, "babel-spread-rest-batch.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
const babelModule = await import("@babel/core");
const spreadModule = await import(${JSON.stringify(spreadName)});
const destructuringModule = await import(${JSON.stringify(destructuringName)});
const parametersModule = await import(${JSON.stringify(parametersName)});
const babel = babelModule.default ?? babelModule;
const spread = spreadModule.default ?? spreadModule;
const destructuring = destructuringModule.default ?? destructuringModule;
const parameters = parametersModule.default ?? parametersModule;
const options = JSON.parse(process.env.MATRIX_BABEL_OPTIONS || "{}");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    const transformOptions = {
      filename: "input.js", babelrc: false, configFile: false, comments: false, compact: false,
      plugins: [
        [spread, options.pluginOptions || {}],
        [destructuring, options.pluginOptions || {}],
        [parameters, options.pluginOptions || {}],
      ],
    };
    if (options.assumptions && Object.keys(options.assumptions).length > 0) {
      transformOptions.assumptions = options.assumptions;
    }
    return { code: babel.transformSync(source, transformOptions).code };
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

// Custom terser-inline transformer for array-destructure-tuple snippet
function terserInlineBatch(sources) {
  const toolDir = ensureNodeTool("terser", ["terser@5"]);
  const helper = join(toolDir, "terser-inline-batch.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
import { minify } from "terser";
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = [];
for (const source of sources) {
  try {
    const result = await minify(source, {
      module: true,
      toplevel: true,
      compress: {
        defaults: true,
        passes: 3,
        inline: true,
        reduce_funcs: true,
        unused: true,
      },
      mangle: { reserved: ["useState", "current", "setCurrent", "use", "value"] },
      format: { comments: false },
    });
    results.push({ code: result.code });
  } catch (e) { results.push({ error: e.message }); }
}
process.stdout.write(JSON.stringify(results));
`,
  );
  const result = spawnSync("node", [helper], {
    cwd: toolDir,
    input: JSON.stringify(sources),
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 50,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`terser batch exited ${result.status}: ${result.stderr}`);
  const outputs = JSON.parse(result.stdout);
  const map = new Map();
  for (let i = 0; i < sources.length; i++) {
    map.set(sources[i], outputs[i].error ? new Error(outputs[i].error) : outputs[i].code);
  }
  return map;
}

const babelSpecRunner = batchRunner(() => babelSpreadRestBatch(allSources, babelProfiles[2], babelModeOptions("spec")));

const transformers = [
  ...babelProfiles.flatMap((profile) =>
    profile.modes.flatMap((mode) =>
      withTerserVariants(
        `${profile.name}-${mode}`,
        allSources,
        batchRunner(() => babelSpreadRestBatch(allSources, profile, babelModeOptions(mode))),
      ),
    ),
  ),
  ...standardLowerers(allSources, { swcExternalHelpers: true }),
];

// Extra per-snippet transformer for terser-inline on array-destructure-tuple
const tupleSnippet = snippets.find((s) => s.name === "array-destructure-tuple");
if (tupleSnippet) {
  tupleSnippet.extraTransformers = [
    {
      name: "babel-7.28-spec-terser-inline",
      run: batchRunner(() => {
        const rawOutputs = allSources.map((s) => { try { return babelSpecRunner(s); } catch { return null; } });
        const valid = rawOutputs.filter((r) => r !== null);
        if (valid.length === 0) return new Map();
        const batchResult = terserInlineBatch(valid);
        const map = new Map();
        for (let i = 0; i < allSources.length; i++) {
          if (rawOutputs[i] !== null) map.set(allSources[i], batchResult.get(rawOutputs[i]));
        }
        return map;
      }),
    },
  ];
}

function validateRecovered({ snippet, shape, recovered }) {
  if (!shape.tools.some((tool) => tool.includes("mangle"))) {
    return undefined;
  }
  const forms = [snippet.source, ...(snippet.acceptForms ?? [])];
  if (matchesAnyForm(recovered, forms)) {
    return { recovered: true, notes: "structurally equivalent to source (mangle-insensitive)" };
  }
  return undefined;
}

async function prewarmComparison(rows) {
  const codes = [];
  for (const { snippet, shape, recovered } of rows) {
    if (recovered == null || !shape.tools.some((tool) => tool.includes("mangle"))) continue;
    codes.push(recovered, snippet.source, ...(snippet.acceptForms ?? []));
  }
  await prewarmNormalize(codes, { rename: true });
}

runMatrix({
  name: "array-spread-rest",
  snippets,
  transformers,
  validateRecovered,
  prewarm: prewarmComparison,
});
