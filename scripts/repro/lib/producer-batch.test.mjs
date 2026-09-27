import assert from "node:assert/strict";
import test from "node:test";
import { babelPresetEnvBatch, parseExactSpec, resolutionCutoff, swcBatch } from "./runner.mjs";

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

test("exact package specs parse; ranges and tags do not", () => {
  assert.deepEqual(parseExactSpec("@babel/core@7.12.17"), { name: "@babel/core", version: "7.12.17" });
  assert.deepEqual(parseExactSpec("@babel/core@8.0.0-rc.5"), { name: "@babel/core", version: "8.0.0-rc.5" });
  assert.deepEqual(parseExactSpec("typescript@5.9.3"), { name: "typescript", version: "5.9.3" });
  assert.equal(parseExactSpec("@swc/core@1"), null);
  assert.equal(parseExactSpec("terser@^5.31.0"), null);
  assert.equal(parseExactSpec("rollup@latest"), null);
  assert.equal(parseExactSpec("typescript"), null);
});

test("the resolution cutoff is one day after the newest publish time", () => {
  assert.equal(
    resolutionCutoff(["2021-02-18T15:13:13.075Z", "2021-02-18T15:13:48.386Z"]),
    "2021-02-19T15:13:48.386Z",
  );
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
