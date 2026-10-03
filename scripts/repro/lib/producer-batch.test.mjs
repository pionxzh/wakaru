import assert from "node:assert/strict";
import test from "node:test";
import { babelPresetEnvBatch, ensureNodeTool, esbuildBatch, swcBatch } from "./runner.mjs";

test("concurrent SWC minifier profiles retain their external-helper setting", async () => {
  const source = "export async function load(value) { return await value; }";
  const options = externalHelpers => ({ minify: true, externalHelpers });
  const external = (await swcBatch([source], options(true))).get(source);
  const inline = (await swcBatch([source], options(false))).get(source);
  assert.match(external, /@swc\/helpers/);
  assert.doesNotMatch(inline, /@swc\/helpers/);

  for (const order of [[true, false], [false, true]]) {
    const outputs = await Promise.all(order.map(value => swcBatch([source], options(value))));
    for (let i = 0; i < order.length; i++) {
      assert.equal(outputs[i].get(source), order[i] ? external : inline);
    }
  }
});

test("SWC and esbuild batches emit CommonJS only when asked", async () => {
  const source = "export let count = 0;";
  assert.match((await swcBatch([source])).get(source), /^export /m);
  assert.match((await swcBatch([source], { moduleType: "commonjs" })).get(source), /\bexports\b/);
  assert.match((await esbuildBatch([source])).get(source), /^export /m);
  assert.match((await esbuildBatch([source], { format: "cjs" })).get(source), /module\.exports/);
});

test("repro tools reject a version range before installing", () => {
  assert.throws(() => ensureNodeTool("range-probe", ["terser@5"]), /terser@5 is not an exact version/);
});

test("a pinned Babel release lowers with its own plugins and helper bodies", async () => {
  // Floating dependencies gave 7.12 core the newest helper bodies (keys routed
  // through `_toPropertyKey`), the newest class plugin (`_callSuper`, 7.23+),
  // and a regenerator plugin that asked 7.12 helpers for a helper they lack.
  const plain = "class Store { get(index) { return index; } }";
  const derived = "class Child extends Base { constructor(value) { super(value); } }";
  const async = "async function load(value) { return await value; }";
  const lowered = await babelPresetEnvBatch([plain, derived, async], { core: "7.12.17", preset: "7.12.17" });
  assert.match(lowered.get(plain), /Object\.defineProperty\(target, descriptor\.key, descriptor\)/);
  assert.doesNotMatch(lowered.get(plain), /_toPropertyKey/);
  assert.match(lowered.get(derived), /function _createSuper\(/);
  assert.doesNotMatch(lowered.get(derived), /_callSuper/);
  assert.match(lowered.get(async), /regeneratorRuntime\.mark/);
});
