import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { ensureNodeTool } from "../lib/runner.mjs";
import {
  assembleCompositionSfc,
  ensureDocsCheckout,
  hasImportRequirement,
  importRequirements,
  normalizeCompiledTemplate,
  toScriptSetup,
} from "./run.mjs";

const toolDir = ensureNodeTool("vue-sfc-3.5.35", [
  "@vue/compiler-sfc@3.5.35",
  "@babel/parser@7.29.7",
]);
const toolRequire = createRequire(join(toolDir, "package.json"));
const babelParser = toolRequire("@babel/parser");

function git(cwd, ...args) {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

const composition = `import DemoGrid from './Grid.vue'
import { ref } from 'vue'

export default {
  components: { DemoGrid },
  setup() {
    const searchQuery = ref('')
    return { searchQuery }
  }
}
`;

const template = `<DemoGrid :filter-key="searchQuery"></DemoGrid>`;

test("converts the docs composition fixture into script setup", () => {
  const script = toScriptSetup(composition, template);
  assert.match(script, /import DemoGrid from '\.\/Grid\.vue'/);
  assert.match(script, /const searchQuery = ref\(''\)/);
  assert.doesNotMatch(script, /export default|components:|return \{/);
});

test("assembles the same SFC shape used by the docs playground", () => {
  const source = assembleCompositionSfc({
    description: "Grid example",
    script: composition,
    template,
    style: ".grid { display: grid; }\n",
  });
  assert.match(source, /^<!--\nGrid example\n-->/);
  assert.match(source, /<script setup>/);
  assert.match(source, /<template>\n  <DemoGrid/);
  assert.match(source, /<style>\n\.grid/);
});

test("accepts a safely renamed local import binding", () => {
  const [requirement] = importRequirements("import { shuffle as _shuffle } from 'lodash-es';");
  assert.equal(
    hasImportRequirement(
      "import { shuffle as shuffle_1 } from 'lodash-es';",
      requirement,
    ),
    true,
  );
  assert.equal(
    hasImportRequirement("import { sample as shuffle_1 } from 'lodash-es';", requirement),
    false,
  );
});

test("normalizes generated-code formatting without changing string contents", () => {
  const formatted = `
    /* generated */
    const _hoisted_1 = { class: "a b" };
    export function render() { return _hoisted_1; }
  `;
  const compact = "const _hoisted_9={class:'a b'};export function render(){return _hoisted_9;}";
  const changedText = "const _hoisted_9={class:'ab'};export function render(){return _hoisted_9;}";

  assert.equal(
    normalizeCompiledTemplate(formatted, babelParser),
    normalizeCompiledTemplate(compact, babelParser),
  );
  assert.notEqual(
    normalizeCompiledTemplate(formatted, babelParser),
    normalizeCompiledTemplate(changedText, babelParser),
  );
});

test("renumbers hoists without collapsing distinct bindings", () => {
  const original = `
    const _hoisted_1 = {};
    const _hoisted_2 = {};
    export function render() { return [_hoisted_1, _hoisted_2]; }
  `;
  const renumbered = `
    const _hoisted_8 = {};
    const _hoisted_3 = {};
    export function render() { return [_hoisted_8, _hoisted_3]; }
  `;
  const swapped = `
    const _hoisted_8 = {};
    const _hoisted_3 = {};
    export function render() { return [_hoisted_3, _hoisted_8]; }
  `;

  assert.equal(
    normalizeCompiledTemplate(original, babelParser),
    normalizeCompiledTemplate(renumbered, babelParser),
  );
  assert.notEqual(
    normalizeCompiledTemplate(original, babelParser),
    normalizeCompiledTemplate(swapped, babelParser),
  );
});

test("pins the managed docs checkout without mutating explicit checkouts", () => {
  const root = mkdtempSync(join(tmpdir(), "wakaru-vue-docs-test-"));
  const source = join(root, "source");
  const managed = join(root, "managed");
  const explicit = join(root, "explicit");
  try {
    git(root, "init", source);
    git(source, "config", "user.name", "Wakaru Test");
    git(source, "config", "user.email", "wakaru@example.test");
    writeFileSync(join(source, "fixture.txt"), "first\n");
    git(source, "add", "fixture.txt");
    git(source, "commit", "-m", "first");
    const pinnedCommit = git(source, "rev-parse", "HEAD");
    writeFileSync(join(source, "fixture.txt"), "second\n");
    git(source, "commit", "-am", "second");
    const latestCommit = git(source, "rev-parse", "HEAD");

    ensureDocsCheckout(managed, {
      commit: pinnedCommit,
      repository: source,
    });
    assert.equal(git(managed, "rev-parse", "HEAD"), pinnedCommit);

    git(managed, "checkout", "--detach", latestCommit);
    ensureDocsCheckout(managed, {
      commit: pinnedCommit,
      repository: source,
    });
    assert.equal(git(managed, "rev-parse", "HEAD"), pinnedCommit);

    git(root, "clone", source, explicit);
    ensureDocsCheckout(explicit, {
      commit: pinnedCommit,
      pin: false,
      repository: source,
    });
    assert.equal(git(explicit, "rev-parse", "HEAD"), latestCommit);

    writeFileSync(join(managed, "fixture.txt"), "dirty\n");
    assert.throws(
      () => ensureDocsCheckout(managed, {
        commit: pinnedCommit,
        repository: source,
      }),
      /refusing to update dirty Vue docs checkout/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
