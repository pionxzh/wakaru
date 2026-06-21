#!/usr/bin/env node

import {
  runMatrix, batchRunner, babelBatch, withTerserVariants, standardLowerers,
} from "../lib/runner.mjs";
import { mangleValidator } from "../lib/compare.mjs";

const snippets = [
  {
    name: "template-basic",
    source: "var out = `Hello ${name}!`;\nuse(out);\n",
    expected: ["`Hello ${name}!`"],
  },
  {
    name: "template-multiple-expressions",
    source: "var out = `${greeting}, ${user.name}! ${count} items`;\nuse(out);\n",
    expected: ["`${greeting}, ${user.name}! ${count} items`"],
  },
  {
    name: "template-expression-start-end",
    source: "var out = `${prefix}/users/${id}`;\nuse(out);\n",
    expected: ["`${prefix}/users/${id}`"],
  },
  {
    name: "template-escaped-newline",
    source: "var out = `line 1\\n${value}\\t${tail}`;\nuse(out);\n",
    expected: ["`line 1\\n${value}\\t${tail}`"],
  },
  {
    name: "template-nested-expression",
    source: "var out = `status: ${ok ? `ok ${name}` : \"bad\"}`;\nuse(out);\n",
    expected: ["`status: ${", "`ok ${name}`"],
  },
  {
    name: "tagged-basic",
    source: "var out = tag`hello ${name}`;\nuse(out);\n",
    expected: ["tag`hello ${name}`"],
  },
  {
    name: "tagged-multiline-newlines",
    source: `var out = tag\`
  staticOne
  staticTwo
  \${dynamicOne}
  \${dynamicTwo}
  staticThree
  \${dynamicThree}
\`;
use(out);
`,
    expected: `tag\`
  staticOne
  staticTwo
  \${dynamicOne}
  \${dynamicTwo}
  staticThree
  \${dynamicThree}
\``,
  },
  {
    name: "tagged-raw-cooked",
    source: "var out = tag`line\\n${value}\\u{1f600}`;\nuse(out);\n",
    expected: ["tag`line\\n${value}\\u{1f600}`"],
  },
  {
    name: "tagged-member",
    source: "var out = css.div`color: ${color}; margin: ${space}px;`;\nuse(out);\n",
    expected: ["css.div`color: ${color}; margin: ${space}px;`"],
  },
];

const babelProfiles = [
  {
    name: "babel-7.8",
    core: "7.8.7",
    plugin: ["@babel/plugin-transform-template-literals", "7.8.3"],
    modes: ["spec", "loose"],
  },
  {
    name: "babel-7.13",
    core: "7.13.16",
    plugin: ["@babel/plugin-transform-template-literals", "7.13.0"],
    modes: ["spec", "loose", "mutableTemplateObject"],
  },
  {
    name: "babel-7.28",
    core: "7.28.5",
    plugin: ["@babel/plugin-transform-template-literals", "7.27.1"],
    modes: ["spec", "loose", "mutableTemplateObject"],
  },
  {
    name: "babel-8-rc",
    core: "8.0.0-rc.5",
    plugin: ["@babel/plugin-transform-template-literals", "8.0.0-rc.5"],
    modes: ["spec", "loose", "mutableTemplateObject"],
  },
];

function babelModeOptions(mode) {
  switch (mode) {
    case "spec":
      return { assumptions: {}, pluginOptions: {} };
    case "loose":
      return { assumptions: {}, pluginOptions: { loose: true } };
    case "mutableTemplateObject":
      return { assumptions: { mutableTemplateObject: true }, pluginOptions: {} };
    default:
      throw new Error(`unsupported Babel mode ${mode}`);
  }
}

const allSources = snippets.map((s) => s.source);

const transformers = [
  ...babelProfiles.flatMap((profile) =>
    profile.modes.flatMap((mode) =>
      withTerserVariants(
        `${profile.name}-${mode}`,
        allSources,
        batchRunner(() => babelBatch(allSources, profile, babelModeOptions(mode))),
      ),
    ),
  ),
  ...standardLowerers(allSources, { esbuildTarget: "es5" }),
];

function expectedNeedles(snippet) {
  return Array.isArray(snippet.expected) ? snippet.expected : [snippet.expected];
}

runMatrix({
  name: "template-literal",
  snippets,
  transformers,
  expectedNeedles,
  ...mangleValidator(),
});
