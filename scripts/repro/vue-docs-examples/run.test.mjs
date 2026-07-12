import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { join } from "node:path";
import test from "node:test";
import { ensureNodeTool } from "../lib/runner.mjs";
import {
  assembleCompositionSfc,
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
