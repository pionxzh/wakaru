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
    name: "numeric-basic",
    source: `
enum Direction {
  Up = 1,
  Down,
  Left = 4,
  Right = -4,
}
use(Direction.Up, Direction[2], Direction.Right);
`,
    expected: ["Up: 1", "Down: 2", "Left: 4", "Right: -4", "2: \"Down\"", "[-4]: \"Right\""],
  },
  {
    name: "numeric-computed",
    source: `
enum Flag {
  None,
  Read = 4,
  Write = 8,
  ReadWrite = Read | Write,
}
use(Flag.None, Flag.ReadWrite);
`,
    expected: ["None: 0", "Read: 4", "Write: 8", "ReadWrite: 12", "12: \"ReadWrite\""],
  },
  {
    name: "numeric-alias-auto-increment",
    source: `
enum Alias {
  First = 1,
  AlsoFirst = First,
  Next,
}
use(Alias.First, Alias.AlsoFirst, Alias.Next, Alias[1], Alias[2]);
`,
    expected: [
      "First: 1",
      "AlsoFirst: 1",
      "Next: 2",
      "1: \"AlsoFirst\"",
      "2: \"Next\"",
    ],
  },
  {
    name: "string-basic",
    source: `
enum Status {
  Ready = "ready",
  Done = "done",
}
use(Status.Ready, Status.Done);
`,
    expected: ["Ready: \"ready\"", "Done: \"done\""],
  },
  {
    name: "heterogeneous",
    source: `
enum Mixed {
  No = 0,
  Yes = "YES",
}
use(Mixed.No, Mixed.Yes, Mixed[0]);
`,
    expected: ["No: 0", "Yes: \"YES\"", "0: \"No\""],
  },
  {
    name: "string-literal-member",
    source: `
enum RenderMode {
  "2D" = 1,
  WebGL = 2,
}
use(RenderMode["2D"], RenderMode.WebGL);
`,
    expected: ["\"2D\": 1", "WebGL: 2", "1: \"2D\"", "2: \"WebGL\""],
  },
  {
    name: "exported-string-enum",
    source: `
export enum Mode {
  Dev = "dev",
  Prod = "prod",
}
use(Mode.Dev);
`,
    expected: ["Mode = {", "Dev: \"dev\"", "Prod: \"prod\""],
  },
];

const babelProfiles = [
  {
    name: "babel-7.8",
    core: "7.8.7",
    plugin: ["@babel/plugin-transform-typescript", "7.8.3"],
    modes: ["standard"],
  },
  {
    name: "babel-7.13",
    core: "7.13.16",
    plugin: ["@babel/plugin-transform-typescript", "7.13.0"],
    modes: ["standard", "optimizeConstEnums"],
  },
  {
    name: "babel-7.28",
    core: "7.28.5",
    plugin: ["@babel/plugin-transform-typescript", "7.28.5"],
    modes: ["standard", "optimizeConstEnums"],
  },
  {
    name: "babel-8",
    core: "8.0.1",
    plugin: ["@babel/plugin-transform-typescript", "8.0.1"],
    modes: ["standard", "optimizeConstEnums"],
  },
];

function babelModeOptions(mode) {
  switch (mode) {
    case "standard":
      return { optimizeConstEnums: false };
    case "optimizeConstEnums":
      return { optimizeConstEnums: true };
    default:
      throw new Error(`unsupported Babel mode ${mode}`);
  }
}

const allSources = snippets.map((s) => s.source);

// Custom babel batch for TypeScript plugin (needs filename: "input.ts")
function babelTsBatch(sources, profile, pluginOptions) {
  const pluginName = profile.plugin[0];
  const pluginVersion = profile.plugin[1];
  const packages = [`@babel/core@${profile.core}`, `${pluginName}@${pluginVersion}`];
  const toolKey = `babel-${profile.core}-transform-typescript`;
  const toolDir = ensureNodeTool(toolKey, packages);
  const helper = join(toolDir, "babel-ts-batch.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
const babelModule = await import("@babel/core");
const pluginModule = await import(${JSON.stringify(pluginName)});
const babel = babelModule.default ?? babelModule;
const plugin = pluginModule.default ?? pluginModule;
const pluginOptions = JSON.parse(process.env.MATRIX_BABEL_OPTIONS || "{}");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: babel.transformSync(source, {
      filename: "input.ts",
      babelrc: false, configFile: false, comments: false, compact: false,
      plugins: [[plugin, pluginOptions]],
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
    env: { ...process.env, MATRIX_BABEL_OPTIONS: JSON.stringify(pluginOptions) },
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

// Custom SWC batch for TypeScript (parser: { syntax: "typescript" }, filename: "input.ts")
function swcTsBatch(sources) {
  const toolDir = ensureNodeTool("swc", ["@swc/core@1"]);
  const helper = join(toolDir, "swc-ts-batch.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const swc = require("@swc/core");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: swc.transformSync(source, {
      filename: "input.ts",
      jsc: { target: "es5", parser: { syntax: "typescript" } },
      module: { type: "es6" },
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

// Custom esbuild batch for TypeScript (loader: "ts")
function esbuildTsBatch(sources) {
  const toolDir = ensureNodeTool("esbuild-0.28", ["esbuild@0.28.0"]);
  const helper = join(toolDir, "esbuild-ts-batch.cjs");
  writeFileSync(
    helper,
    `
const fs = require("node:fs");
const esbuild = require("esbuild");
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map(source => {
  try {
    return { code: esbuild.transformSync(source, {
      loader: "ts", target: "es2015", format: "esm", logLevel: "warning",
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
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`esbuild batch exited ${result.status}: ${result.stderr}`);
  const outputs = JSON.parse(result.stdout);
  const map = new Map();
  for (let i = 0; i < sources.length; i++) {
    map.set(sources[i], outputs[i].error ? new Error(outputs[i].error) : outputs[i].code);
  }
  return map;
}

const transformers = [
  ...babelProfiles.flatMap((profile) =>
    profile.modes.flatMap((mode) =>
      withTerserVariants(
        `${profile.name}-${mode}`,
        allSources,
        batchRunner(() => babelTsBatch(allSources, profile, babelModeOptions(mode))),
      ),
    ),
  ),
  ...standardLowerers(allSources, {
    swc: (sources) => swcTsBatch(sources),
    esbuild: (sources) => esbuildTsBatch(sources),
    includeSource: false,
  }),
];

runMatrix({
  name: "enum",
  snippets,
  transformers,
  ...mangleValidator(),
});
