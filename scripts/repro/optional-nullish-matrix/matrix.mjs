#!/usr/bin/env node

import {
  runMatrix, batchRunner, withTerserVariants,
  ensureNodeTool, readOption, standardLowerers,
} from "../lib/runner.mjs";
import { join } from "node:path";
import { writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const snippets = [
  {
    name: "member-chain-nullish",
    source: "const out = obj?.foo?.bar ?? fallback;\n",
    expected: ["?.", "??"],
  },
  {
    name: "mixed-leading-required-members",
    source: "const out = obj.foo?.bar.baz?.qux;\n",
    expected: ["obj.foo?.bar.baz?.qux"],
  },
  {
    name: "mixed-leading-optional-member",
    source: "const out = obj?.foo.bar?.baz.qux;\n",
    expected: ["obj?.foo.bar?.baz.qux"],
  },
  {
    name: "optional-call-nullish",
    source: "const out = obj?.method?.(arg) ?? fallback;\n",
    expected: ["?.", "??"],
  },
  {
    name: "nested-receiver-call",
    source: "const out = obj?.foo?.method?.(arg);\n",
    expected: ["?.method?.("],
  },
  {
    name: "computed-member-nullish",
    source: "const out = obj?.[key]?.value ?? fallback;\n",
    expected: ["?.[", "??"],
  },
  {
    name: "logical-and-negated-member-call",
    source: "const hidden = !settings?.role?.blocks?.hideList.includes(blockName);\n",
    expected: ["!settings?.role?.blocks?.hideList.includes(blockName)"],
  },
  {
    name: "logical-and-duplicated-access",
    source:
      'if (root?.[key]?.group?.items && root?.[key]?.group?.items.length) {\n  sink("clear");\n} else {\n  sink("fill");\n}\n',
    expected: ["root?.[key]?.group?.items && root?.[key]?.group?.items.length"],
  },
  {
    name: "logical-and-prefix-loose-comparisons",
    source:
      'if (item?.meta?.enabled && item.kind != "alpha" && item.kind != "beta" && item.kind != "gamma") {\n  sink(item);\n}\n',
    expected: ['item?.meta?.enabled && item.kind != "alpha"'],
  },
  {
    name: "logical-and-prefix-strict-comparisons",
    source:
      'if (item?.meta?.enabled && item.kind !== "alpha" && item.kind !== "beta" && item.kind !== "gamma") {\n  sink(item);\n}\n',
    expected: ['item?.meta?.enabled && item.kind !== "alpha"'],
  },
  {
    name: "logical-and-prefix-includes-condition",
    source:
      'const blockedKinds = ["alpha", "beta", "gamma"];\nif (item?.meta?.enabled && !blockedKinds.includes(item.kind)) {\n  sink(item);\n}\n',
    expected: ["item?.meta?.enabled && !blockedKinds.includes(item.kind)"],
  },
  {
    name: "nullish-only",
    source: "const out = value ?? fallback;\n",
    expected: ["??"],
  },
  {
    name: "nullish-with-optional-middle",
    source: "const out = foo ?? bar?.prop ?? baz;\n",
    expected: ["foo ??", "bar?.prop ?? baz"],
  },
  {
    name: "optional-after-nullish",
    source: "const out = (obj?.foo ?? fallback)?.bar;\n",
    expected: ["??", "?.bar"],
  },
];

const babelProfiles = [
  {
    name: "babel-7.8",
    core: "7.8.7",
    optionalPlugin: ["@babel/plugin-proposal-optional-chaining", "7.8.3"],
    nullishPlugin: ["@babel/plugin-proposal-nullish-coalescing-operator", "7.8.3"],
    modes: ["spec", "loose"],
  },
  {
    name: "babel-7.13",
    core: "7.13.16",
    optionalPlugin: ["@babel/plugin-proposal-optional-chaining", "7.13.12"],
    nullishPlugin: ["@babel/plugin-proposal-nullish-coalescing-operator", "7.13.8"],
    modes: ["spec", "noDocumentAll", "loose"],
  },
  {
    name: "babel-7.28",
    core: "7.28.5",
    optionalPlugin: ["@babel/plugin-transform-optional-chaining", "7.28.5"],
    nullishPlugin: ["@babel/plugin-transform-nullish-coalescing-operator", "7.28.6"],
    modes: ["spec", "noDocumentAll", "loose"],
  },
  {
    name: "babel-8-rc",
    core: "8.0.0-rc.5",
    optionalPlugin: ["@babel/plugin-transform-optional-chaining", "8.0.0-rc.5"],
    nullishPlugin: ["@babel/plugin-transform-nullish-coalescing-operator", "8.0.0-rc.5"],
    modes: ["spec", "noDocumentAll", "loose"],
  },
];

function babelModeOptions(mode) {
  switch (mode) {
    case "spec":
      return { assumptions: {}, pluginOptions: {} };
    case "noDocumentAll":
      return { assumptions: { noDocumentAll: true }, pluginOptions: {} };
    case "loose":
      return { assumptions: {}, pluginOptions: { loose: true } };
    default:
      throw new Error(`unsupported Babel mode ${mode}`);
  }
}

function babelOptionalNullishBatch(sources, profile, options) {
  const [optionalName, optionalVersion] = profile.optionalPlugin;
  const [nullishName, nullishVersion] = profile.nullishPlugin;
  const packages = [
    `@babel/core@${profile.core}`,
    `${optionalName}@${optionalVersion}`,
    `${nullishName}@${nullishVersion}`,
  ];
  const toolKey = `babel-${profile.core}-optional-chaining-nullish-coalescing`;
  const toolDir = ensureNodeTool(toolKey, packages);
  const helper = join(toolDir, "babel-optional-nullish-batch.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
const babelModule = await import("@babel/core");
const optionalModule = await import(${JSON.stringify(optionalName)});
const nullishModule = await import(${JSON.stringify(nullishName)});
const babel = babelModule.default ?? babelModule;
const optional = optionalModule.default ?? optionalModule;
const nullish = nullishModule.default ?? nullishModule;
const options = JSON.parse(process.env.MATRIX_BABEL_OPTIONS || "{}");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    const transformOptions = {
      filename: "input.js", babelrc: false, configFile: false, comments: false, compact: false,
      plugins: [
        [optional, options.pluginOptions || {}],
        [nullish, options.pluginOptions || {}],
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
const rewriteLevel = readOption("--level", "standard");

function expectedNeedles(snippet) {
  const needles = Array.isArray(snippet.expected) ? snippet.expected : [snippet.expected];
  return needles;
}

const transformers = [
  ...babelProfiles.flatMap((profile) =>
    profile.modes.flatMap((mode) =>
      withTerserVariants(
        `${profile.name}-${mode}`,
        allSources,
        batchRunner(() => babelOptionalNullishBatch(allSources, profile, babelModeOptions(mode))),
      ),
    ),
  ),
  ...standardLowerers(allSources),
];

runMatrix({
  name: "optional-nullish",
  snippets,
  transformers,
});
