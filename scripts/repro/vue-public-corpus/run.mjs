#!/usr/bin/env node

import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { ensureNodeTool } from "../lib/runner.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "../../..");
const corpusRoot = join(repoRoot, "target", "vue-public-corpus");
const sourceRoot = join(corpusRoot, "sources");
const outputRoot = join(corpusRoot, "outputs");
const casesPath = join(scriptDir, "cases.json");

main().catch((error) => {
  console.error(error?.stack ?? String(error));
  process.exitCode = 1;
});

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const cases = readCases();

  if (options.help) {
    printHelp();
    return;
  }
  if (options.list) {
    printCases(cases);
    return;
  }

  const selected = selectCases(cases, options);
  if (selected.length === 0) {
    throw new Error("no cases selected");
  }

  mkdirSync(corpusRoot, { recursive: true });
  mkdirSync(sourceRoot, { recursive: true });
  mkdirSync(outputRoot, { recursive: true });

  const wakaru = resolveWakaru(options);
  const rows = [];

  for (const testCase of selected) {
    const started = Date.now();
    try {
      console.error(`== ${testCase.name} ==`);
      const row = runCase(testCase, options, wakaru);
      row.elapsed_ms = Date.now() - started;
      rows.push(row);
    } catch (error) {
      const row = {
        name: testCase.name,
        tier: testCase.tier ?? "",
        bundler: testCase.bundler ?? "",
        ref: testCase.ref,
        status: "error",
        error: error.message,
        elapsed_ms: Date.now() - started,
      };
      rows.push(row);
      if (!options.keepGoing) {
        break;
      }
    }
  }

  const report = {
    generated_at: new Date().toISOString(),
    wakaru,
    cases: rows,
  };
  const markdown = formatMarkdownReport(report);
  writeFileSync(join(corpusRoot, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
  writeFileSync(join(corpusRoot, "report.md"), markdown);

  if (options.json) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log(markdown);
  }

  if (rows.some((row) => row.status === "error")) {
    process.exitCode = 1;
  }
}

function parseArgs(args) {
  const options = {
    all: false,
    caseNames: [],
    clean: false,
    help: false,
    json: false,
    keepGoing: true,
    list: false,
    refresh: false,
    skipBuild: false,
    skipInstall: false,
    skipSfcValidate: false,
    skipWakaruBuild: false,
  };

  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === "--all") options.all = true;
    else if (arg === "--case") options.caseNames.push(readArgValue(args, ++i, "--case"));
    else if (arg.startsWith("--case=")) options.caseNames.push(arg.slice("--case=".length));
    else if (arg === "--clean") options.clean = true;
    else if (arg === "--help" || arg === "-h") options.help = true;
    else if (arg === "--json") options.json = true;
    else if (arg === "--list") options.list = true;
    else if (arg === "--no-keep-going") options.keepGoing = false;
    else if (arg === "--refresh") options.refresh = true;
    else if (arg === "--skip-build") options.skipBuild = true;
    else if (arg === "--skip-install") options.skipInstall = true;
    else if (arg === "--skip-sfc-validate") options.skipSfcValidate = true;
    else if (arg === "--no-build-wakaru") options.skipWakaruBuild = true;
    else throw new Error(`unknown option ${arg}`);
  }

  return options;
}

function readArgValue(args, index, flag) {
  const value = args[index];
  if (!value || value.startsWith("--")) {
    throw new Error(`${flag} requires a value`);
  }
  return value;
}

function readCases() {
  const cases = JSON.parse(readFileSync(casesPath, "utf8"));
  for (const testCase of cases) {
    for (const field of ["name", "repo", "ref", "install", "build", "inputs"]) {
      if (testCase[field] === undefined) {
        throw new Error(`case ${testCase.name ?? "<unnamed>"} is missing ${field}`);
      }
    }
  }
  return cases;
}

function selectCases(cases, options) {
  if (options.caseNames.length > 0) {
    const selected = [];
    for (const name of options.caseNames) {
      const found = cases.find((testCase) => testCase.name === name);
      if (!found) {
        const available = cases.map((testCase) => testCase.name).join(", ");
        throw new Error(`unknown case ${name} (available: ${available})`);
      }
      selected.push(found);
    }
    return selected;
  }
  if (options.all) {
    return cases;
  }
  return cases.filter((testCase) => testCase.enabled !== false);
}

function printHelp() {
  console.log(`Usage: node scripts/repro/vue-public-corpus/run.mjs [options]

Options:
  --list                 List configured cases.
  --case <name>          Run one case. May be passed multiple times.
  --all                  Include disabled cases.
  --refresh              Fetch the pinned ref even when the checkout exists.
  --clean                Remove selected source checkouts before running.
  --skip-install         Skip the case install command.
  --skip-build           Skip the case build command.
  --skip-sfc-validate    Skip @vue/compiler-sfc validation.
  --no-build-wakaru      Use WAKARU or an existing dev-release binary.
  --no-keep-going        Stop after the first failed case.
  --json                 Print JSON report instead of Markdown.
  --help                 Show this message.
`);
}

function printCases(cases) {
  console.log("| case | enabled | tier | bundler | ref | notes |");
  console.log("|---|---:|---|---|---|---|");
  for (const testCase of cases) {
    console.log(
      `| ${cell(testCase.name)} | ${testCase.enabled !== false ? "yes" : "no"} | ${cell(testCase.tier ?? "")} | ${cell(testCase.bundler ?? "")} | ${cell(testCase.ref)} | ${cell(testCase.notes ?? "")} |`,
    );
  }
}

function resolveWakaru(options) {
  if (process.env.WAKARU) {
    return process.env.WAKARU;
  }

  const binary = join(repoRoot, "target", "dev-release", process.platform === "win32" ? "wakaru.exe" : "wakaru");
  if (!options.skipWakaruBuild) {
    runChecked(["cargo", "build", "--profile", "dev-release", "-p", "wakaru-cli"], {
      cwd: repoRoot,
      label: "build wakaru-cli",
    });
  }
  if (!existsSync(binary)) {
    throw new Error(`wakaru binary not found at ${binary}; build it or set WAKARU`);
  }
  return binary;
}

function runCase(testCase, options, wakaru) {
  const checkout = ensureCheckout(testCase, options);
  const workingDir = join(checkout, testCase.subdir ?? "");
  if (!existsSync(workingDir)) {
    throw new Error(`case working directory does not exist: ${workingDir}`);
  }

  if (!options.skipInstall) {
    runChecked(testCase.install, {
      cwd: workingDir,
      label: `${testCase.name} install`,
    });
  }
  if (!options.skipBuild) {
    runChecked(testCase.build, {
      cwd: workingDir,
      label: `${testCase.name} build`,
    });
  }

  const inputs = testCase.inputs.map((input) => join(workingDir, input));
  for (const input of inputs) {
    if (!existsSync(input)) {
      throw new Error(`case input does not exist after build: ${input}`);
    }
  }

  const outputDir = join(outputRoot, testCase.name);
  rmSync(outputDir, { recursive: true, force: true });
  mkdirSync(outputDir, { recursive: true });

  const wakaruResult = runCapture(wakaru, [
    "--unpack",
    "--vue-sfc",
    "--json",
    "--force",
    "-o",
    outputDir,
    ...inputs,
  ], {
    cwd: repoRoot,
    label: `${testCase.name} wakaru`,
  });
  const json = JSON.parse(wakaruResult.stdout);
  const vueFiles = listFiles(outputDir).filter((file) => file.endsWith(".vue"));
  const unsupported = countUnsupportedMarkers(vueFiles);
  const validation = options.skipSfcValidate
    ? { total: vueFiles.length, parse_ok: 0, template_ok: 0, errors: [] }
    : validateVueSfcFiles(vueFiles, outputDir);

  const counts = countStatuses(json.modules ?? []);
  return {
    name: testCase.name,
    tier: testCase.tier ?? "",
    bundler: testCase.bundler ?? "",
    ref: testCase.ref,
    status: "ok",
    detected_formats: json.detected_formats ?? [],
    total: json.total ?? 0,
    failed: json.failed ?? 0,
    decompiled: counts.decompiled ?? 0,
    vue_sfc_source_js: counts.vue_sfc_source_js ?? 0,
    recovered_vue_sfc: counts.recovered_vue_sfc ?? 0,
    vue_sfc_fallback_js: counts.vue_sfc_fallback_js ?? 0,
    vue_files: vueFiles.length,
    unsupported_markers: unsupported,
    sfc_parse_ok: validation.parse_ok,
    sfc_template_ok: validation.template_ok,
    sfc_validation_errors: validation.errors,
    warnings: json.warnings ?? [],
    output_dir: relative(repoRoot, outputDir).replaceAll("\\", "/"),
    notes: testCase.notes ?? "",
  };
}

function ensureCheckout(testCase, options) {
  const checkout = join(sourceRoot, testCase.name);
  if (options.clean) {
    rmSync(checkout, { recursive: true, force: true });
  }

  const gitDir = join(checkout, ".git");
  if (!existsSync(gitDir)) {
    rmSync(checkout, { recursive: true, force: true });
    mkdirSync(checkout, { recursive: true });
    runChecked(["git", "init"], { cwd: checkout, label: `${testCase.name} git init` });
    runChecked(["git", "remote", "add", "origin", testCase.repo], {
      cwd: checkout,
      label: `${testCase.name} git remote add`,
    });
    configureSparseCheckout(checkout, testCase.sparse);
    fetchRef(checkout, testCase);
  } else if (options.refresh) {
    fetchRef(checkout, testCase);
  }

  return checkout;
}

function configureSparseCheckout(checkout, sparse) {
  if (!Array.isArray(sparse) || sparse.length === 0) {
    return;
  }
  runChecked(["git", "config", "core.sparseCheckout", "true"], {
    cwd: checkout,
    label: "enable sparse checkout",
  });
  const infoDir = join(checkout, ".git", "info");
  mkdirSync(infoDir, { recursive: true });
  const patterns = sparse.map((path) => `/${path.replaceAll("\\", "/")}/**`).join("\n");
  writeFileSync(join(infoDir, "sparse-checkout"), `${patterns}\n`);
}

function fetchRef(checkout, testCase) {
  runChecked(["git", "fetch", "--depth", "1", "origin", testCase.ref], {
    cwd: checkout,
    label: `${testCase.name} git fetch`,
  });
  runChecked(["git", "checkout", "--detach", "FETCH_HEAD"], {
    cwd: checkout,
    label: `${testCase.name} git checkout`,
  });
}

function countStatuses(modules) {
  const counts = {};
  for (const module of modules) {
    counts[module.status] = (counts[module.status] ?? 0) + 1;
  }
  return counts;
}

function countUnsupportedMarkers(files) {
  let total = 0;
  for (const file of files) {
    const source = readFileSync(file, "utf8");
    total += source.match(/<!--\s*wakaru:/g)?.length ?? 0;
  }
  return total;
}

function validateVueSfcFiles(files, outputDir) {
  if (files.length === 0) {
    return { total: 0, parse_ok: 0, template_ok: 0, errors: [] };
  }

  const toolDir = ensureNodeTool("vue-public-corpus-sfc-3.5.35", ["@vue/compiler-sfc@3.5.35"]);
  const helper = join(toolDir, "validate-sfc.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
import { parse, compileTemplate } from "@vue/compiler-sfc";

const files = JSON.parse(fs.readFileSync(0, "utf8"));
const rows = files.map((file, index) => {
  const source = fs.readFileSync(file, "utf8");
  const errors = [];
  let descriptor;
  try {
    const parsed = parse(source, { filename: file });
    descriptor = parsed.descriptor;
    for (const error of parsed.errors) {
      errors.push(error.message || String(error));
    }
  } catch (error) {
    errors.push(error.message || String(error));
  }

  let templateOk = true;
  if (descriptor?.template) {
    try {
      const compiled = compileTemplate({
        source: descriptor.template.content,
        filename: file,
        id: "wakaru-public-corpus-" + index.toString(36),
      });
      for (const error of compiled.errors) {
        errors.push(error.message || String(error));
      }
      templateOk = compiled.errors.length === 0;
    } catch (error) {
      errors.push(error.message || String(error));
      templateOk = false;
    }
  }

  return {
    file,
    parse_ok: descriptor !== undefined && errors.length === 0,
    template_ok: descriptor !== undefined && templateOk,
    errors,
  };
});

process.stdout.write(JSON.stringify(rows));
`,
  );

  const result = runCapture("node", [helper], {
    cwd: toolDir,
    input: JSON.stringify(files),
    label: "validate recovered vue sfc",
  });
  const rows = JSON.parse(result.stdout);
  return {
    total: rows.length,
    parse_ok: rows.filter((row) => row.parse_ok).length,
    template_ok: rows.filter((row) => row.template_ok).length,
    errors: rows
      .filter((row) => row.errors.length > 0)
      .map((row) => ({
        file: relative(outputDir, row.file).replaceAll("\\", "/"),
        errors: row.errors,
      })),
  };
}

function listFiles(root) {
  if (!existsSync(root)) {
    return [];
  }
  const files = [];
  for (const entry of readdirSync(root)) {
    const path = join(root, entry);
    const stat = statSync(path);
    if (stat.isDirectory()) {
      files.push(...listFiles(path));
    } else if (stat.isFile()) {
      files.push(path);
    }
  }
  return files;
}

function formatMarkdownReport(report) {
  const lines = [];
  lines.push("# Vue public corpus report");
  lines.push("");
  lines.push(`- generated: ${report.generated_at}`);
  lines.push(`- wakaru: ${report.wakaru}`);
  lines.push("");
  lines.push("| case | status | bundler | formats | modules | recovered | fallback | unsupported | sfc parse | template compile | elapsed |");
  lines.push("|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|");
  for (const row of report.cases) {
    if (row.status === "error") {
      lines.push(
        `| ${cell(row.name)} | error | ${cell(row.bundler)} |  | 0 | 0 | 0 | 0 | 0 | 0 | ${row.elapsed_ms}ms |`,
      );
      continue;
    }
    lines.push(
      `| ${cell(row.name)} | ok | ${cell(row.bundler)} | ${cell(row.detected_formats.join(", "))} | ${row.total} | ${row.recovered_vue_sfc} | ${row.vue_sfc_fallback_js} | ${row.unsupported_markers} | ${row.sfc_parse_ok}/${row.vue_files} | ${row.sfc_template_ok}/${row.vue_files} | ${row.elapsed_ms}ms |`,
    );
  }

  const errors = report.cases.filter((row) => row.status === "error");
  const validationErrors = report.cases.flatMap((row) =>
    (row.sfc_validation_errors ?? []).map((error) => ({ case: row.name, ...error })),
  );
  if (errors.length > 0 || validationErrors.length > 0) {
    lines.push("");
    lines.push("## Details");
    for (const row of errors) {
      lines.push("");
      lines.push(`- ${row.name}: ${row.error}`);
    }
    for (const error of validationErrors) {
      lines.push("");
      lines.push(`- ${error.case}/${error.file}: ${error.errors.join("; ")}`);
    }
  }

  lines.push("");
  return `${lines.join("\n")}\n`;
}

function cell(value) {
  return String(value).replaceAll("|", "\\|").replaceAll("\n", " ");
}

function runChecked(command, options = {}) {
  const result = spawnCommand(command, {
    cwd: options.cwd,
    input: options.input,
    stdio: options.input === undefined ? "inherit" : "pipe",
  });
  if (result.status !== 0) {
    throwCommandError(command, result, options.label);
  }
  return result.stdout ?? "";
}

function runCapture(command, argsOrOptions = {}, maybeOptions = {}) {
  const commandArray = Array.isArray(command) ? command : [command, ...(Array.isArray(argsOrOptions) ? argsOrOptions : [])];
  const options = Array.isArray(argsOrOptions) ? maybeOptions : argsOrOptions;
  const result = spawnCommand(commandArray, {
    cwd: options.cwd,
    input: options.input,
    stdio: "pipe",
  });
  if (result.status !== 0) {
    throwCommandError(commandArray, result, options.label);
  }
  return { stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

function spawnCommand(command, options = {}) {
  const [rawCommand, ...rawArgs] = command;
  const [resolvedCommand, resolvedArgs] = resolveCommand(rawCommand, rawArgs);
  return spawnSync(resolvedCommand, resolvedArgs, {
    cwd: options.cwd ?? repoRoot,
    input: options.input,
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 50,
    shell: false,
    stdio: options.stdio ?? "pipe",
    env: process.env,
  });
}

function resolveCommand(command, args) {
  if (process.platform !== "win32") {
    return [command, args];
  }
  if (!["corepack", "npm", "npx", "pnpm", "yarn"].includes(command)) {
    return [command, args];
  }
  return ["cmd.exe", ["/d", "/s", "/c", `${command}.cmd`, ...args]];
}

function throwCommandError(command, result, label) {
  if (result.error) {
    throw result.error;
  }
  const name = label ?? command.join(" ");
  const detail = [result.stderr?.trim(), result.stdout?.trim()].filter(Boolean).join(" ");
  throw new Error(`${name} exited ${result.status}${detail ? `: ${detail}` : ""}`);
}
