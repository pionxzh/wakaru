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
import { createRequire } from "node:module";
import { dirname, join, relative, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { ensureNodeTool } from "../lib/runner.mjs";
import {
  compileVueSfc,
  VUE_SFC_COMPILE_PROFILES,
  vueSfcCompileProfile,
} from "../lib/vue-sfc-compiler.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "../../..");
const defaultDocsRoot = join(repoRoot, "target", "vue-docs");
const outputRoot = join(repoRoot, "target", "vue-docs-examples");

export function toScriptSetup(source, template) {
  const exportDefaultIndex = source.indexOf("export default");
  const lastReturnIndex = source.lastIndexOf("return {");
  let setupCode = lastReturnIndex > -1
    ? deindent(
        source
          .slice(exportDefaultIndex, lastReturnIndex)
          .replace(/export default[^]+?setup\([^)]*\)\s*{/, "")
          .trim(),
      )
    : "";

  const propsStartIndex = source.indexOf("\n  props:");
  if (propsStartIndex > -1) {
    const propsEndIndex = source.indexOf("\n  }", propsStartIndex) + 4;
    const propsVar = /\bprops\b/.test(template) || /\bprops\b/.test(source)
      ? "const props = "
      : "";
    const propsDef = deindent(
      source
        .slice(propsStartIndex, propsEndIndex)
        .trim()
        .replace(/,$/, "")
        .replace(/^props: /, `${propsVar}defineProps(`) + ")",
      1,
    );
    setupCode = `${propsDef}\n\n${setupCode}`.trim();
  }

  const emitsStartIndex = source.indexOf("\n  emits:");
  if (emitsStartIndex > -1) {
    const emitsEndIndex = source.indexOf("]", emitsStartIndex) + 1;
    const emitsDef = source
      .slice(emitsStartIndex, emitsEndIndex)
      .trim()
      .replace(/,$/, "")
      .replace(/^emits: /, "const emit = defineEmits(") + ")";
    setupCode = `${emitsDef}\n\n${setupCode}`.trim();
  }

  const prefix = source.slice(0, exportDefaultIndex);
  const result = prefix + setupCode;
  return `${setupCode ? result : result.trim()}\n`;
}

export function assembleCompositionSfc({ description = "", script, template, style = "" }) {
  let source = description.trim() ? `<!--\n${description.trim()}\n-->\n\n` : "";
  source += `<script setup>\n${toScriptSetup(script, template)}</script>\n\n`;
  source += `<template>\n${indent(template)}</template>`;
  if (style) source += `\n\n<style>\n${style}</style>`;
  return source;
}

function indent(source) {
  return source
    .split("\n")
    .map((line) => line.trim() ? `  ${line}` : line)
    .join("\n");
}

function deindent(source, tabSize = 2) {
  return source
    .split("\n")
    .map((line) => line.replace(tabSize === 1 ? /^\s{2}/ : /^\s{4}/, ""))
    .join("\n");
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  const docsRoot = resolve(options.docsRoot ?? defaultDocsRoot);
  ensureDocsCheckout(docsRoot);
  const examplesRoot = join(docsRoot, "src", "examples", "src");
  if (!existsSync(examplesRoot)) {
    throw new Error(`Vue docs examples not found at ${examplesRoot}`);
  }

  const docsPackage = JSON.parse(readFileSync(join(docsRoot, "package.json"), "utf8"));
  const vueVersion = docsPackage.dependencies.vue.replace(/^[^\d]*/, "");
  const toolDir = ensureNodeTool(
    `vue-docs-examples-sfc-${vueVersion}`,
    [`@vue/compiler-sfc@${vueVersion}`],
  );
  const require = createRequire(join(toolDir, "package.json"));
  const compiler = require("@vue/compiler-sfc");
  const wakaru = resolveWakaru(options);
  const profiles = options.profile
    ? [vueSfcCompileProfile(options.profile)]
    : VUE_SFC_COMPILE_PROFILES;

  rmSync(outputRoot, { recursive: true, force: true });
  mkdirSync(outputRoot, { recursive: true });
  const generatedRoot = join(outputRoot, "generated");
  const recoveredRoot = join(outputRoot, "recovered");
  mkdirSync(generatedRoot, { recursive: true });
  mkdirSync(recoveredRoot, { recursive: true });

  const fixtures = readFixtures(examplesRoot)
    .filter((fixture) => !options.filter || fixture.name.includes(options.filter));
  const rows = profiles.flatMap((profile) =>
    fixtures.map((fixture) =>
      runFixture({
        fixture,
        profile,
        compiler,
        wakaru,
        generatedRoot,
        recoveredRoot,
      }),
    ),
  );
  const docsCommit = runCapture(["git", "-C", docsRoot, "rev-parse", "HEAD"]).trim();
  const report = {
    docs_commit: docsCommit,
    vue_version: vueVersion,
    profiles: profiles.map(({ name, tier, isProd, inlineTemplate }) => ({
      name,
      tier,
      is_prod: isProd,
      inline_template: inlineTemplate,
    })),
    rows,
  };
  writeFileSync(join(outputRoot, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
  writeFileSync(join(outputRoot, "report.md"), formatReport(report));
  process.stdout.write(options.json
    ? `${JSON.stringify(report, null, 2)}\n`
    : formatReport(report));
}

function parseArgs(args) {
  const options = {
    docsRoot: undefined,
    filter: "",
    profile: "",
    json: false,
    skipWakaruBuild: false,
  };
  for (let index = 0; index < args.length; index++) {
    const arg = args[index];
    if (arg === "--docs") options.docsRoot = args[++index];
    else if (arg === "--filter") options.filter = args[++index] ?? "";
    else if (arg === "--profile") options.profile = args[++index] ?? "";
    else if (arg === "--json") options.json = true;
    else if (arg === "--no-build-wakaru") options.skipWakaruBuild = true;
    else throw new Error(`unknown option ${arg}`);
  }
  return options;
}

function ensureDocsCheckout(docsRoot) {
  if (existsSync(join(docsRoot, ".git"))) return;
  mkdirSync(dirname(docsRoot), { recursive: true });
  runChecked(["git", "clone", "--depth", "1", "git@github.com:vuejs/docs.git", docsRoot]);
}

function resolveWakaru(options) {
  const binary = process.env.WAKARU || join(
    repoRoot,
    "target",
    "debug",
    process.platform === "win32" ? "wakaru.exe" : "wakaru",
  );
  if (!options.skipWakaruBuild && !process.env.WAKARU) {
    runChecked(["cargo", "build", "-p", "wakaru-cli"], { cwd: repoRoot });
  }
  if (!existsSync(binary)) throw new Error(`Wakaru binary not found at ${binary}`);
  return binary;
}

function readFixtures(examplesRoot) {
  const fixtures = [];
  for (const example of readdirSync(examplesRoot).sort()) {
    const exampleRoot = join(examplesRoot, example);
    if (!statSync(exampleRoot).isDirectory()) continue;
    const descriptionPath = join(exampleRoot, "description.txt");
    const description = existsSync(descriptionPath) ? readFileSync(descriptionPath, "utf8") : "";
    for (const component of readdirSync(exampleRoot).sort()) {
      const componentRoot = join(exampleRoot, component);
      if (!statSync(componentRoot).isDirectory()) continue;
      const compositionPath = join(componentRoot, "composition.js");
      const templatePath = join(componentRoot, "template.html");
      if (!existsSync(compositionPath) || !existsSync(templatePath)) continue;
      const stylePath = join(componentRoot, "style.css");
      fixtures.push({
        name: `${example}/${component}`,
        filename: `${component}.vue`,
        source: assembleCompositionSfc({
          description: component === "App" ? description : "",
          script: readFileSync(compositionPath, "utf8"),
          template: readFileSync(templatePath, "utf8"),
          style: existsSync(stylePath) ? readFileSync(stylePath, "utf8") : "",
        }),
      });
    }
  }
  return fixtures;
}

function runFixture({
  fixture,
  profile,
  compiler,
  wakaru,
  generatedRoot,
  recoveredRoot,
}) {
  const generated = compileVueSfc({
    source: fixture.source,
    filename: fixture.filename,
    compiler,
    profile,
    id: `data-v-docs-${fixture.filename.replace(/\W/g, "-")}`,
    includeFilename: !profile.isProd,
  });
  const generatedPath = join(generatedRoot, profile.name, `${fixture.name}.js`);
  const recoveredPath = join(recoveredRoot, profile.name, `${fixture.name}.vue`);
  mkdirSync(dirname(generatedPath), { recursive: true });
  mkdirSync(dirname(recoveredPath), { recursive: true });
  writeFileSync(generatedPath, generated);

  const result = spawnSync(wakaru, [
    generatedPath,
    "--vue-sfc",
    "--force",
    "-o",
    recoveredPath,
  ], { cwd: repoRoot, encoding: "utf8", maxBuffer: 1024 * 1024 * 20 });
  const recovered = result.status === 0 && existsSync(recoveredPath)
    ? readFileSync(recoveredPath, "utf8")
    : "";
  const validation = validateRecovered(fixture.source, recovered, fixture.filename, compiler);
  return {
    name: fixture.name,
    profile: profile.name,
    profile_tier: profile.tier,
    recovered: Boolean(recovered),
    exit_code: result.status,
    stderr: result.stderr.trim(),
    ...validation,
  };
}

function validateRecovered(original, recovered, filename, compiler) {
  if (!recovered) {
    return {
      parse_ok: false,
      template_ok: false,
      script_setup: false,
      missing_imports: [],
      leaked_markers: [],
      template_equivalent: false,
    };
  }
  const originalParsed = compiler.parse(original, { filename }).descriptor;
  const parsed = compiler.parse(recovered, { filename });
  const parseOk = parsed.errors.length === 0;
  const descriptor = parsed.descriptor;
  let templateOk = false;
  let templateEquivalent = false;
  if (parseOk && descriptor.template) {
    const recoveredTemplate = compileTemplateForComparison(descriptor.template.content, filename, compiler);
    const originalTemplate = compileTemplateForComparison(
      originalParsed.template.content,
      filename,
      compiler,
    );
    templateOk = recoveredTemplate.errors.length === 0;
    templateEquivalent = templateOk
      && normalizeCompiledTemplate(recoveredTemplate.code)
        === normalizeCompiledTemplate(originalTemplate.code);
  }
  const recoveredScript = [descriptor.script?.content, descriptor.scriptSetup?.content]
    .filter(Boolean)
    .join("\n");
  const originalImports = importRequirements(originalParsed.scriptSetup?.content ?? "");
  const missingImports = originalImports
    .filter((requirement) => !hasImportRequirement(recoveredScript, requirement))
    .map((requirement) => `${requirement.imported} from ${requirement.source}`);
  const leakedMarkers = ["$setup", "__isScriptSetup", "__expose"]
    .filter((marker) => recovered.includes(marker));
  return {
    parse_ok: parseOk,
    template_ok: templateOk,
    script_setup: Boolean(descriptor.scriptSetup),
    missing_imports: missingImports,
    leaked_markers: leakedMarkers,
    template_equivalent: templateEquivalent,
  };
}

function compileTemplateForComparison(source, filename, compiler) {
  return compiler.compileTemplate({
    source,
    filename,
    id: "data-v-docs-compare",
    isProd: true,
    compilerOptions: { hoistStatic: true },
  });
}

function normalizeCompiledTemplate(code) {
  return code.replace(/\/\*[^]*?\*\//g, "").replace(/\s+/g, "").replace(/_hoisted_\d+/g, "_hoisted");
}

export function importRequirements(source) {
  const requirements = [];
  const pattern = /import\s+([^;\n]+?)\s+from\s+["']([^"']+)["']/g;
  for (const match of source.matchAll(pattern)) {
    const clause = match[1].trim();
    const sourceName = match[2];
    const defaultMatch = clause.match(/^([A-Za-z_$][\w$]*)/);
    if (defaultMatch) {
      requirements.push({ kind: "default", imported: "default", local: defaultMatch[1], source: sourceName });
    }
    const namespaceMatch = clause.match(/\*\s+as\s+([A-Za-z_$][\w$]*)/);
    if (namespaceMatch) {
      requirements.push({ kind: "namespace", imported: "*", local: namespaceMatch[1], source: sourceName });
    }
    const named = clause.match(/\{([^}]*)\}/)?.[1] ?? "";
    for (const specifier of named.split(",").map((part) => part.trim()).filter(Boolean)) {
      const parts = specifier.split(/\s+as\s+/);
      requirements.push({
        kind: "named",
        imported: parts[0].trim(),
        local: parts.at(-1).trim(),
        source: sourceName,
      });
    }
  }
  return requirements;
}

export function hasImportRequirement(script, requirement) {
  return importRequirements(script).some((candidate) =>
    candidate.kind === requirement.kind
      && candidate.imported === requirement.imported
      && candidate.source === requirement.source
  );
}

function formatReport(report) {
  const passed = report.rows.filter((row) =>
    row.recovered
      && row.parse_ok
      && row.template_ok
      && row.script_setup
      && row.missing_imports.length === 0
      && row.leaked_markers.length === 0,
  ).length;
  const equivalent = report.rows.filter((row) => row.template_equivalent).length;
  const lines = [
    "# Vue docs examples recovery",
    `# docs: ${report.docs_commit}`,
    `# Vue compiler: ${report.vue_version}`,
    `# acceptance: ${passed}/${report.rows.length}`,
    `# template-equivalent: ${equivalent}/${report.rows.length}`,
  ];
  for (const profile of report.profiles) {
    const rows = report.rows.filter((row) => row.profile === profile.name);
    const profilePassed = rows.filter((row) =>
      row.recovered
        && row.parse_ok
        && row.template_ok
        && row.script_setup
        && row.missing_imports.length === 0
        && row.leaked_markers.length === 0,
    ).length;
    const profileEquivalent = rows.filter((row) => row.template_equivalent).length;
    lines.push(
      `# ${profile.name} (${profile.tier}): ${profilePassed}/${rows.length} accepted, ${profileEquivalent}/${rows.length} template-equivalent`,
    );
  }
  lines.push(
    "",
    "| profile | component | recovered | valid | script setup | imports | template | leaks |",
    "|---|---|---:|---:|---:|---:|---:|---|",
  );
  for (const row of report.rows) {
    lines.push(`| ${row.profile} | ${row.name} | ${yes(row.recovered)} | ${yes(row.parse_ok && row.template_ok)} | ${yes(row.script_setup)} | ${row.missing_imports.length ? row.missing_imports.join(", ") : "yes"} | ${yes(row.template_equivalent)} | ${row.leaked_markers.join(", ")} |`);
  }
  return `${lines.join("\n")}\n`;
}

function yes(value) {
  return value ? "yes" : "no";
}

function runChecked(command, options = {}) {
  const result = spawnSync(command[0], command.slice(1), {
    cwd: options.cwd ?? repoRoot,
    encoding: "utf8",
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command.join(" ")} exited ${result.status}`);
}

function runCapture(command) {
  const result = spawnSync(command[0], command.slice(1), { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command.join(" ")} exited ${result.status}`);
  return result.stdout;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(error?.stack ?? String(error));
    process.exitCode = 1;
  }
}
