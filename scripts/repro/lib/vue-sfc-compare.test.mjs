import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { join } from "node:path";
import test from "node:test";
import { ensureNodeTool } from "./runner.mjs";
import { linkedEventHandlerProgram } from "./vue-sfc-compare.mjs";

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
