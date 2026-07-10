import assert from "node:assert/strict";
import test from "node:test";

import {
  parseTestMetadata,
  runnableVariants,
  Test262MetadataError,
} from "./test262-metadata.mjs";

test("parses typed metadata fields from inline and block YAML forms", () => {
  const metadata = parseTestMetadata(`/*---
esid: sec-example
features: [optional-chaining, "Symbol.iterator"]
includes:
  - propertyHelper.js
  - 'compareArray.js'
flags: [onlyStrict, async]
negative:
  phase: runtime
  type: TypeError
---*/
`);

  assert.equal(metadata.esid, "sec-example");
  assert.deepEqual(metadata.features, ["optional-chaining", "Symbol.iterator"]);
  assert.deepEqual(metadata.includes, ["propertyHelper.js", "compareArray.js"]);
  assert.deepEqual(metadata.flags, ["onlyStrict", "async"]);
  assert.deepEqual(metadata.negative, { phase: "runtime", type: "TypeError" });
});

test("accepts every known flag and negative phase", () => {
  for (const flag of [
    "onlyStrict",
    "noStrict",
    "module",
    "raw",
    "async",
    "generated",
    "CanBlockIsFalse",
    "CanBlockIsTrue",
    "non-deterministic",
    "explicit-resource-management",
  ]) {
    assert.equal(parseTestMetadata(`/*---\nflags: [${flag}]\n---*/`).flags[0], flag);
  }
  for (const phase of ["parse", "early", "resolution", "runtime"]) {
    assert.equal(
      parseTestMetadata(
        `/*---\nnegative: { phase: ${phase}, type: SyntaxError }\n---*/`,
      ).negative.phase,
      phase,
    );
  }
});

test("rejects missing, malformed, and unknown metadata", () => {
  for (const [source, pattern] of [
    ["void 0;", /missing Test262 metadata start marker/],
    ["/*---\nflags: [onlyStrict]", /missing Test262 metadata end marker/],
    ["/*---\nflags: [futureFlag]\n---*/", /unknown Test262 flag/],
    ["/*---\nnegative: { phase: future, type: Error }\n---*/", /unknown Test262 negative phase/],
    ["/*---\nnegative:\n  phase: parse\n---*/", /requires phase and type/],
    ["/*---\nflags: onlyStrict\n---*/", /must be a YAML sequence/],
  ]) {
    assert.throws(() => parseTestMetadata(source), pattern);
  }
});

test("rejects conflicting and duplicate flags", () => {
  for (const source of [
    "/*---\nflags: [onlyStrict, noStrict]\n---*/",
    "/*---\nflags: [raw, onlyStrict]\n---*/",
    "/*---\nflags: [module, noStrict]\n---*/",
    "/*---\nflags: [async, async]\n---*/",
  ]) {
    assert.throws(() => parseTestMetadata(source), Test262MetadataError);
  }
});

test("creates script, module, and raw variants", () => {
  assert.deepEqual(runnableVariants({ flags: [] }), [
    { name: "sloppy", strict: false },
    { name: "strict", strict: true },
  ]);
  assert.deepEqual(runnableVariants({ flags: ["module"] }), [
    { name: "module", strict: true, module: true },
  ]);
  assert.deepEqual(runnableVariants({ flags: ["raw"] }), [
    { name: "raw-script", strict: false, raw: true },
  ]);
  assert.deepEqual(runnableVariants({ flags: ["module", "raw"] }), [
    { name: "raw-module", strict: true, module: true, raw: true },
  ]);
});
