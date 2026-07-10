#!/usr/bin/env node

import {
  runMatrix, batchRunner, withTerserVariants, ensureNodeTool,
} from "../lib/runner.mjs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { writeFileSync } from "node:fs";

const VUE_COMPILER_VERSION = "3.5.35";

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
    expected: [
      "<template>",
      "<button",
      ":class=\"{ active: props.active }\"",
      "@click=\"increment\"",
      "{{ props.count }}",
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
    expected: ["<PanelHeader :title=\"title\" />", "<slot name=\"body\">Empty</slot>"],
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
    expected: ["v-model=\"value\"", "v-show=\"visible\""],
  },
];

const allSources = snippets.map((s) => s.source);

function vueSfcBatch(sources, options = {}) {
  const isProd = options.isProd ?? true;
  const toolKey = `vue-sfc-${VUE_COMPILER_VERSION}`;
  const toolDir = ensureNodeTool(toolKey, [`@vue/compiler-sfc@${VUE_COMPILER_VERSION}`]);
  const helper = join(toolDir, "vue-sfc-batch.mjs");
  writeFileSync(
    helper,
    `
import fs from "node:fs";
import { parse, compileScript, compileTemplate } from "@vue/compiler-sfc";

const isProd = process.env.MATRIX_VUE_PROD === "1";
const sources = JSON.parse(fs.readFileSync(0, "utf8"));
const results = sources.map((source, index) => {
  const filename = "Component" + index + ".vue";
  const id = "data-v-wakaru-" + index.toString(36);
  try {
    const parsed = parse(source, { filename });
    if (parsed.errors.length > 0) {
      return { error: parsed.errors.map(error => error.message || String(error)).join("; ") };
    }

    const descriptor = parsed.descriptor;
    const script = descriptor.script || descriptor.scriptSetup
      ? compileScript(descriptor, {
          id,
          genDefaultAs: "__sfc__",
        }).content
      : "const __sfc__ = {}";

    if (!descriptor.template) {
      return { code: script + "\\nexport default __sfc__;\\n" };
    }

    const template = compileTemplate({
      source: descriptor.template.content,
      filename,
      id,
      isProd,
      compilerOptions: {
        hoistStatic: true,
        cacheHandlers: true,
      },
    });
    if (template.errors.length > 0) {
      return { error: template.errors.map(error => error.message || String(error)).join("; ") };
    }

    return {
      code: [
        script,
        template.code,
        "__sfc__.render = render;",
        "export default __sfc__;",
      ].join("\\n\\n"),
    };
  } catch (error) {
    return { error: error.message };
  }
});
process.stdout.write(JSON.stringify(results));
`,
  );

  const result = spawnSync("node", [helper], {
    cwd: toolDir,
    input: JSON.stringify(sources),
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 50,
    env: { ...process.env, MATRIX_VUE_PROD: isProd ? "1" : "0" },
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

const transformers = [
  ...withTerserVariants(
    `vue-${VUE_COMPILER_VERSION}-prod`,
    allSources,
    batchRunner(() => vueSfcBatch(allSources, { isProd: true })),
  ),
  ...withTerserVariants(
    `vue-${VUE_COMPILER_VERSION}-dev`,
    allSources,
    batchRunner(() => vueSfcBatch(allSources, { isProd: false })),
  ),
];

runMatrix({
  name: "vue-render",
  snippets,
  transformers,
  wakaruArgs: ["--vue-sfc"],
});
