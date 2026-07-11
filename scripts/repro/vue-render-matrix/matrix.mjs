#!/usr/bin/env node

import {
  runMatrix, batchRunner, withTerserVariants, ensureNodeTool,
} from "../lib/runner.mjs";
import { prewarmNormalize, structurallyEqual } from "../lib/compare.mjs";
import { VUE_SFC_COMPILE_PROFILES } from "../lib/vue-sfc-compiler.mjs";
import { linkedEventHandlerProgram } from "../lib/vue-sfc-compare.mjs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";

const VUE_COMPILER_VERSION = "3.5.35";
const BABEL_PARSER_VERSION = "7.29.7";
const vueToolDir = ensureNodeTool(
  `vue-sfc-${VUE_COMPILER_VERSION}`,
  [
    `@vue/compiler-sfc@${VUE_COMPILER_VERSION}`,
    `@babel/parser@${BABEL_PARSER_VERSION}`,
  ],
);
const vueToolRequire = createRequire(join(vueToolDir, "package.json"));
const vueCompareOptions = {
  compiler: vueToolRequire("@vue/compiler-sfc"),
  babelParser: vueToolRequire("@babel/parser"),
};

const snippets = [
  {
    name: "static-element",
    source: `
<template>
  <section class="card">Hello Vue</section>
</template>
`,
    expected: ["<template>", "<section class=\"card\">Hello Vue</section>"],
  },
  {
    name: "text-interpolation",
    source: `
<script>
export default {
  props: {
    msg: String
  }
}
</script>
<template>
  <div>{{ msg }}</div>
</template>
`,
    expected: ["<template>", "<div>{{ msg }}</div>"],
  },
  {
    name: "script-setup-event-and-class",
    source: `
<script setup>
const props = defineProps({
  active: Boolean,
  count: Number
})
const emit = defineEmits(["increment"])
function increment() {
  emit("increment")
}
</script>
<template>
  <button class="counter" :class="{ active: props.active }" @click="increment">
    {{ props.count }}
  </button>
</template>
`,
    expectedAny: [
      [
        "<script setup>",
        "const props = defineProps(",
        "defineEmits(",
        ":class=\"{ active: props.active }\"",
        "@click=",
        "{{ props.count }}",
      ],
      [
        "<script setup>",
        "const props = defineProps(",
        "defineEmits(",
        ":class=\"{ active }\"",
        "@click=",
        "{{ count }}",
      ],
      [
        "<script setup>",
        "const __props = defineProps(",
        "const props = __props;",
        "defineEmits(",
        ":class=\"{ active: props.active }\"",
        "@click=",
        "{{ props.count }}",
      ],
    ],
  },
  {
    name: "conditional-branches",
    source: `
<script setup>
defineProps({
  status: String,
  error: String
})
</script>
<template>
  <p v-if="status === 'loading'">Loading</p>
  <p v-else-if="status === 'error'">{{ error }}</p>
  <p v-else>Ready</p>
</template>
`,
    expected: ["v-if=\"status === 'loading'\"", "v-else-if=\"status === 'error'\"", "v-else"],
  },
  {
    name: "list-render",
    source: `
<script setup>
defineProps({
  items: Array
})
</script>
<template>
  <ul>
    <li v-for="item in items" :key="item.id">{{ item.name }}</li>
  </ul>
</template>
`,
    expected: ["v-for=\"item in items\"", ":key=\"item.id\"", "{{ item.name }}"],
  },
  {
    name: "component-and-slot",
    source: `
<script setup>
import PanelHeader from "./PanelHeader.vue"
defineProps({
  title: String
})
</script>
<template>
  <article>
    <PanelHeader :title="title" />
    <slot name="body">Empty</slot>
  </article>
</template>
`,
    expected: [
      "<script setup>",
      "import PanelHeader from \"./PanelHeader.vue\";",
      "<PanelHeader :title=\"title\" />",
      "<slot name=\"body\">Empty</slot>",
    ],
  },
  {
    name: "scoped-slots-with-destructuring",
    source: `
<script setup>
import DataList from "./DataList.vue"
defineProps({ items: Array })
function select(item) {
  return item.id
}
</script>
<template>
  <DataList :items="items">
    <template #default="{ item, index }">
      <button :data-index="index" @click="select(item)">{{ item.name }}</button>
    </template>
    <template #empty>No items</template>
  </DataList>
</template>
`,
    expected: [
      "v-slot:default=\"{ item, index }\"",
      ":data-index=\"index\"",
      "@click=\"select(item)\"",
      "{{ item.name }}",
      "v-slot:empty",
    ],
  },
  {
    name: "model-and-directive",
    source: `
<script setup>
const value = defineModel()
const visible = true
</script>
<template>
  <input v-model="value" v-show="visible" />
</template>
`,
    expected: ["<script setup>", "v-model=", "v-show="],
  },
];

const allSources = snippets.map((s) => s.source);

function vueSfcBatch(sources, profile) {
  const helper = join(vueToolDir, "vue-sfc-batch.mjs");
  const compilerHelper = pathToFileURL(
    join(dirname(fileURLToPath(import.meta.url)), "..", "lib", "vue-sfc-compiler.mjs"),
  ).href;
  writeFileSync(
    helper,
    `
import fs from "node:fs";
import { parse, compileScript, compileTemplate } from "@vue/compiler-sfc";
import { compileVueSfc } from ${JSON.stringify(compilerHelper)};

const profile = JSON.parse(process.env.MATRIX_VUE_PROFILE);
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map((source, index) => {
  const filename = "Component" + index + ".vue";
  const id = "data-v-wakaru-" + index.toString(36);
  try {
    return {
      code: compileVueSfc({
        source,
        filename,
        compiler: { parse, compileScript, compileTemplate },
        profile,
        id,
      }),
    };
  } catch (error) {
    return { error: error.message };
  }
});
process.stdout.write(JSON.stringify(results));
`,
  );

  const result = spawnSync("node", [helper], {
    cwd: vueToolDir,
    input: JSON.stringify(sources),
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 50,
    env: { ...process.env, MATRIX_VUE_PROFILE: JSON.stringify(profile) },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = [result.stderr.trim(), result.stdout.trim()].filter(Boolean).join(" ");
    throw new Error(`vue compiler batch exited ${result.status}: ${detail}`);
  }
  const outputs = JSON.parse(result.stdout);
  const map = new Map();
  for (let i = 0; i < sources.length; i++) {
    map.set(sources[i], outputs[i].error ? new Error(outputs[i].error) : outputs[i].code);
  }
  return map;
}

const transformers = VUE_SFC_COMPILE_PROFILES.flatMap((profile) =>
  withTerserVariants(
    `vue-${VUE_COMPILER_VERSION}-${profile.name}`,
    allSources,
    batchRunner(() => vueSfcBatch(allSources, profile)),
  ),
);

function linkedScopedSlotPrograms(snippet, recovered) {
  if (snippet.name !== "scoped-slots-with-destructuring") return null;
  const sourceProgram = linkedEventHandlerProgram(snippet.source, vueCompareOptions);
  const recoveredProgram = linkedEventHandlerProgram(recovered, vueCompareOptions);
  if (!sourceProgram || !recoveredProgram) return null;
  return { sourceProgram, recoveredProgram };
}

runMatrix({
  name: "vue-render",
  snippets,
  transformers,
  wakaruArgs: ["--vue-sfc"],
  validateRecovered({ snippet, shape, recovered }) {
    if (shape.tools.some((tool) => tool.includes("mangle"))) {
      const linked = linkedScopedSlotPrograms(snippet, recovered);
      const eventNeedle = linked && `@click="${linked.sourceProgram.expression}"`;
      const otherSyntaxPresent = linked && snippet.expected
        .filter((needle) => needle !== eventNeedle)
        .every((needle) => recovered.includes(needle));
      if (
        linked
        && otherSyntaxPresent
        && structurallyEqual(linked.sourceProgram.program, linked.recoveredProgram.program)
      ) {
        return {
          recovered: true,
          notes: "linked slot handler is structurally equivalent (mangle-insensitive)",
        };
      }
    }
    if (snippet.name === "model-and-directive") {
      const modelBinding = recovered.match(
        /v-model(?:\.[^=]+)?="([A-Za-z_$][\w$]*)(?:\.value)?"/,
      );
      if (!modelBinding || recovered.includes(`v-model="${modelBinding[1]}.value"`)) {
        return {
          recovered: false,
          notes: "template v-model still contains compiled ref .value access",
        };
      }
      const declaration = new RegExp(
        `const\\s+${modelBinding[1]}\\s*=\\s*useModel\\(`,
      );
      if (!declaration.test(recovered)) {
        return {
          recovered: false,
          notes: `v-model binding ${modelBinding[1]} has no recovered useModel declaration`,
        };
      }
    }
  },
  async prewarm(rows) {
    const codes = [];
    for (const { snippet, shape, recovered } of rows) {
      if (recovered == null || !shape.tools.some((tool) => tool.includes("mangle"))) continue;
      const linked = linkedScopedSlotPrograms(snippet, recovered);
      if (linked) codes.push(linked.sourceProgram.program, linked.recoveredProgram.program);
    }
    await prewarmNormalize(codes, { rename: true });
  },
});
