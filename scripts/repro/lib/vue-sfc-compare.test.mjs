import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { join } from "node:path";
import test from "node:test";
import { ensureNodeTool } from "./runner.mjs";
import {
  linkedEventHandlerProgram,
  setupDirectiveBinding,
} from "./vue-sfc-compare.mjs";

const toolDir = ensureNodeTool("vue-sfc-3.5.35", [
  "@vue/compiler-sfc@3.5.35",
  "@babel/parser@7.29.7",
]);
const toolRequire = createRequire(join(toolDir, "package.json"));
const options = {
  compiler: toolRequire("@vue/compiler-sfc"),
  babelParser: toolRequire("@babel/parser"),
};

const original = `
<script setup>
function select(item) {
  return item.id
}
</script>
<template>
  <DataList>
    <template #default="{ item, index }">
      <button @click="select(item)">{{ item.name }}</button>
    </template>
  </DataList>
</template>
`;

function recovered(expression = "c(item)") {
  return `
<script setup>
function c(t) {
  return t.id;
}
</script>
<template>
  <DataList>
    <template v-slot:default="{ item, index }">
      <button @click="${expression}">{{ item.name }}</button>
    </template>
  </DataList>
</template>
`;
}

test("links renamed setup handlers to ordered scoped-slot bindings", () => {
  const sourceProgram = linkedEventHandlerProgram(original, options);
  const recoveredProgram = linkedEventHandlerProgram(recovered(), options);

  assert.deepEqual(sourceProgram.scopeBindings, ["item", "index"]);
  assert.deepEqual(recoveredProgram.scopeBindings, ["item", "index"]);
  assert.equal(sourceProgram.handlerName, "select");
  assert.equal(recoveredProgram.handlerName, "c");
  assert.match(sourceProgram.program, /function select\(item\)/);
  assert.match(sourceProgram.program, /\(item, index\) => \(select\(item\)\)/);
  assert.match(recoveredProgram.program, /function c\(t\)/);
  assert.match(recoveredProgram.program, /\(item, index\) => \(c\(item\)\)/);
});

test("preserves which scoped-slot binding the handler receives", () => {
  const correct = linkedEventHandlerProgram(recovered(), options);
  const wrong = linkedEventHandlerProgram(recovered("c(index)"), options);

  assert.match(correct.program, /\(item, index\) => \(c\(item\)\)/);
  assert.match(wrong.program, /\(item, index\) => \(c\(index\)\)/);
  assert.notEqual(correct.program, wrong.program);
});

test("declines events that cannot be linked to a setup declaration", () => {
  assert.equal(
    linkedEventHandlerProgram(
      `<script setup>const other = 1</script>
       <template><template #default="{ item }"><button @click="missing(item)" /></template></template>`,
      options,
    ),
    null,
  );
});

test("accepts only plain directive expressions linked to setup bindings", () => {
  const source = (event, shown = "visible") => `
    <script setup>
    const visible = true
    function ${event === "onClick" ? "onClick" : "handler"}() {}
    </script>
    <template><button @click="${event}" v-show="${shown}" /></template>
  `;

  assert.deepEqual(
    setupDirectiveBinding(source("onClick"), {
      ...options,
      directiveName: "on",
      argument: "click",
    }),
    { name: "onClick", kind: "function", initializer: null },
  );
  assert.deepEqual(
    setupDirectiveBinding(source("onClick"), {
      ...options,
      directiveName: "show",
    }),
    { name: "visible", kind: "variable", initializer: null },
  );
  assert.equal(
    setupDirectiveBinding(source("_cache[0]"), {
      ...options,
      directiveName: "on",
      argument: "click",
    }),
    null,
  );
  assert.equal(
    setupDirectiveBinding(source("onClick", "_ctx.visible"), {
      ...options,
      directiveName: "show",
    }),
    null,
  );
});

test("reports the setup initializer used by a directive binding", () => {
  for (const initializer of ["useModel", "defineModel"]) {
    const source = `
      <script setup>const value = ${initializer}()</script>
      <template><input v-model="value" /></template>
    `;
    assert.deepEqual(
      setupDirectiveBinding(source, {
        ...options,
        directiveName: "model",
      }),
      { name: "value", kind: "variable", initializer },
    );
  }
});
