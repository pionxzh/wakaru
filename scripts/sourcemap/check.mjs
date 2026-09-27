#!/usr/bin/env node
// Measure an output source map at token level: which output tokens carry a
// mapping, and whether each mapping lands on a matching input token.
//
//   node scripts/sourcemap/check.mjs <input.js> <output.js> [--map <file>]
//   node scripts/sourcemap/check.mjs <bundle.js> <unpack-out-dir>
//
// With a directory, every `*.js` that has a `.js.map` beside it is checked
// against the same input and the results are summed. Options:
//   --map <file>    map for a single output (default: <output>.map)
//   --last          resolve duplicate segments to the last one (default: first,
//                   as @jridgewell/trace-mapping does)
//   --examples <n>  examples per mismatch pair (default 2)
//   --top <n>       mismatch pairs to list (default 20)
//   --json          print the result as JSON
//
// Most non-matching pairs are faithful rewrites (`const <- var`, `true <- !0`,
// an `if` mapped to the expression it replaced). Look for pairs that cannot
// be a rewrite of the mapped input token, and treat any `(no token)` target
// as a broken offset.

import { readdirSync, readFileSync, statSync, existsSync } from "node:fs";
import { createRequire } from "node:module";
import { join, relative } from "node:path";
import { ensureNodeTool } from "../repro/lib/runner.mjs";
import { analyzeMap, mergeResults, tokenKind } from "./quality.mjs";

const TOOL_PACKAGES = ["acorn@8.18.0", "acorn-jsx@5.3.2", "@jridgewell/trace-mapping@0.3.31"];

function parseArgs(argv) {
  const options = { pick: "first", examples: 2, top: 20, json: false, map: null };
  const positional = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--last") options.pick = "last";
    else if (arg === "--json") options.json = true;
    else if (arg === "--map") options.map = argv[++i];
    else if (arg === "--examples") options.examples = Number(argv[++i]);
    else if (arg === "--top") options.top = Number(argv[++i]);
    else if (arg.startsWith("--")) throw new Error(`unknown option ${arg}`);
    else positional.push(arg);
  }
  if (positional.length !== 2) {
    throw new Error("usage: check.mjs <input.js> <output.js | output-dir> [--map file] [--last] [--json]");
  }
  [options.input, options.output] = positional;
  return options;
}

function loadTools() {
  const toolDir = ensureNodeTool("sourcemap-check-1", TOOL_PACKAGES);
  const require = createRequire(join(toolDir, "package.json"));
  const acorn = require("acorn");
  return {
    Parser: acorn.Parser.extend(require("acorn-jsx")()),
    traceMapping: require("@jridgewell/trace-mapping"),
  };
}

// Tokens the emitter can map. Empty template chunks and whitespace-only JSX
// text print nothing, so no emitter maps them; counting them would report a
// coverage gap that does not exist.
function tokenize(Parser, code) {
  const collect = (sourceType) => {
    const tokens = [];
    Parser.parse(code, {
      ecmaVersion: "latest",
      sourceType,
      locations: true,
      allowHashBang: true,
      allowReturnOutsideFunction: true,
      allowAwaitOutsideFunction: true,
      allowImportExportEverywhere: true,
      onToken(token) {
        const label = token.type.label;
        if (label === "eof" || token.start === token.end) return;
        if (label === "jsxText" && !token.value.trim()) return;
        tokens.push({
          kind: tokenKind({ label, keyword: token.type.keyword, value: token.value }),
          value: token.value,
          line: token.loc.start.line - 1,
          col: token.loc.start.column,
        });
      },
    });
    return tokens;
  };
  try {
    return collect("module");
  } catch {
    return collect("script");
  }
}

function readSegments(traceMapping, mapJson) {
  const segments = [];
  traceMapping.eachMapping(new traceMapping.TraceMap(mapJson), (mapping) => {
    if (mapping.source == null) return;
    segments.push({
      genLine: mapping.generatedLine - 1,
      genCol: mapping.generatedColumn,
      srcLine: mapping.originalLine - 1,
      srcCol: mapping.originalColumn,
    });
  });
  return segments;
}

function outputsToCheck(output, explicitMap) {
  if (!statSync(output).isDirectory()) {
    return [{ js: output, map: explicitMap ?? `${output}.map` }];
  }
  return readdirSync(output, { recursive: true })
    .map(String)
    .filter((file) => file.endsWith(".js") && existsSync(join(output, `${file}.map`)))
    .sort()
    .map((file) => ({ js: join(output, file), map: join(output, `${file}.map`) }));
}

const pct = (part, whole) => (whole ? `${((100 * part) / whole).toFixed(1)}%` : "n/a");

function snippet(lines, { line, col }) {
  const text = lines[line] ?? "";
  return JSON.stringify(text.slice(Math.max(0, col - 20), col + 30));
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  const { Parser, traceMapping } = loadTools();
  const input = readFileSync(options.input, "utf8");
  const inputTokens = tokenize(Parser, input);
  const inputLines = input.split(/\r\n|\r|\n|\u2028|\u2029/);

  const results = [];
  const outputLines = new Map();
  const skipped = [];
  for (const { js, map } of outputsToCheck(options.output, options.map)) {
    const code = readFileSync(js, "utf8");
    let outputTokens;
    try {
      outputTokens = tokenize(Parser, code);
    } catch (error) {
      skipped.push(`${js}: ${error.message}`);
      continue;
    }
    const segments = readSegments(traceMapping, readFileSync(map, "utf8"));
    const result = analyzeMap({ outputTokens, inputTokens, segments }, options);
    const file = statSync(options.output).isDirectory() ? relative(options.output, js) : js;
    for (const pair of result.pairs) {
      for (const example of pair.examples) example.file = file;
    }
    outputLines.set(file, code.split("\n"));
    results.push(result);
  }
  const total = mergeResults(results, options);

  if (options.json) {
    console.log(JSON.stringify({ files: results.length, skipped, ...total }, null, 2));
    return;
  }

  const checked = total.mapped;
  console.log(`files checked: ${results.length}${skipped.length ? `, skipped: ${skipped.length}` : ""}`);
  console.log(`output tokens: ${total.outputTokens}, segments: ${total.segments}`);
  console.log(`mapped at token start: ${pct(total.mapped, total.outputTokens)}`);
  console.log(`resolvable by lookup:  ${pct(total.resolvable, total.outputTokens)}`);
  console.log(`duplicate positions:   ${total.duplicatePositions}`);
  console.log("coverage by kind:");
  for (const [group, stats] of Object.entries(total.byKind).sort((a, b) => b[1].total - a[1].total)) {
    console.log(`  ${group.padEnd(8)} ${pct(stats.mapped, stats.total).padStart(6)} of ${stats.total}`);
  }
  console.log(`mapped tokens (${options.pick} segment per position):`);
  console.log(`  same token     ${pct(total.compared.same, checked)}`);
  console.log(`  renamed ident  ${pct(total.compared.renamed, checked)}`);
  console.log(`  other token    ${pct(total.compared.different, checked)}`);
  console.log(`  no input token ${pct(total.compared.offToken, checked)}`);
  if (total.pairs.length) console.log("\nmost frequent non-matching pairs (output <- input):");
  for (const pair of total.pairs.slice(0, options.top)) {
    console.log(`  ${String(pair.count).padStart(7)}  ${pair.key}`);
    for (const example of pair.examples) {
      const lines = outputLines.get(example.file) ?? [];
      const where = results.length > 1 ? `${example.file} ` : "";
      console.log(`           out ${where}${example.output.line + 1}:${example.output.col} ${snippet(lines, example.output)}`);
      console.log(`            in ${example.input.line + 1}:${example.input.col} ${snippet(inputLines, example.input)}`);
    }
  }
  for (const reason of skipped) console.error(`skipped ${reason}`);
}

main();
