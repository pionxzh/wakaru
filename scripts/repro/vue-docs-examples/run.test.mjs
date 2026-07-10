import assert from "node:assert/strict";
import test from "node:test";
import {
  assembleCompositionSfc,
  hasImportRequirement,
  importRequirements,
  toScriptSetup,
} from "./run.mjs";

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
