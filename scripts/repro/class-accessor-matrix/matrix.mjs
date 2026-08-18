#!/usr/bin/env node

import {
  batchRunner,
  esbuildBatch,
  runMatrix,
  swcBatch,
  tscBatch,
  withTerserVariants,
} from "../lib/runner.mjs";
import { mangleValidator } from "../lib/compare.mjs";

// Class syntax cannot express a zero-argument setter, but descriptor callbacks
// can and do occur in real bundles. These inputs start at that legal ES5 shape
// and exercise the distinct class-recovery paths after production minifiers.
const descriptorSnippets = [
  {
    name: "prototype-zero-arg-setter",
    source: `
function Widget() {}
Object.defineProperty(Widget.prototype, "value", {
  enumerable: false,
  configurable: true,
  get: function() { return this._value; },
  set: function() {}
});
var widget = new Widget();
var descriptor = Object.getOwnPropertyDescriptor(Widget.prototype, "value");
use(descriptor.enumerable, descriptor.configurable, widget.value);
widget.value = 1;
`,
    expected: ["class ", "get value()", "set value(", ".value = 1"],
    rejected: ["set value()"],
    execute: {},
  },
  {
    name: "iife-zero-arg-setter",
    source: `
var Widget = (function() {
  function Inner() {}
  Object.defineProperty(Inner.prototype, "text", {
    enumerable: false,
    configurable: true,
    get: function() { return null; },
    set: function() {}
  });
  return Inner;
}());
var first = new Widget();
var second = new Widget();
var descriptor = Object.getOwnPropertyDescriptor(Widget.prototype, "text");
use(descriptor.enumerable, descriptor.configurable, first.text, second.text);
first.text = "ignored";
second.text = "ignored";
`,
    expected: ["class ", "get text()", "set text(", ".text = \"ignored\""],
    rejected: ["set text()"],
    execute: {},
  },
  {
    name: "iife-mixed-accessor-syntax",
    source: `
var Widget = (function() {
  function Inner() {}
  Object.defineProperty(Inner.prototype, "value", {
    configurable: true,
    get: function() { return null; },
    set() {}
  });
  return Inner;
}());
var widget = new Widget();
use(widget.value);
widget.value = 1;
`,
    expected: ["class ", "get value()", "set value(", ".value = 1"],
    rejected: ["set value()", "Object.defineProperty("],
    execute: {},
  },
  {
    name: "create-class-zero-arg-setter",
    source: `
var _createClass = function() {
  function defineProperties(target, descriptors) {
    for (var index = 0; index < descriptors.length; index++) {
      var descriptor = descriptors[index];
      descriptor.enumerable = descriptor.enumerable || false;
      descriptor.configurable = true;
      "value" in descriptor && (descriptor.writable = true);
      Object.defineProperty(target, descriptor.key, descriptor);
    }
  }
  return function(Constructor, prototypeDescriptors, staticDescriptors) {
    prototypeDescriptors && defineProperties(Constructor.prototype, prototypeDescriptors);
    staticDescriptors && defineProperties(Constructor, staticDescriptors);
    return Constructor;
  };
}();
var Widget = (function() {
  function Inner() {}
  _createClass(Inner, [
    { key: "text", get: function() { return null; } },
    { key: "text", set: function() {} }
  ]);
  return Inner;
}());
var first = new Widget();
var second = new Widget();
var descriptor = Object.getOwnPropertyDescriptor(Widget.prototype, "text");
use(descriptor.enumerable, descriptor.configurable, first.text, second.text);
first.text = "ignored";
second.text = "ignored";
`,
    expected: ["class ", "get text()", "set text(", ".text = \"ignored\""],
    rejected: ["set text()", "_createClass("],
    execute: {},
  },
  {
    name: "multi-param-setter-fails-closed",
    source: `
function Widget() {}
Object.defineProperty(Widget.prototype, "value", {
  enumerable: false,
  configurable: true,
  set: function(value, metadata) { use(value, metadata); }
});
var widget = new Widget();
widget.value = 1;
`,
    expected: ["Object.defineProperty(", "set ("],
    rejected: ["class "],
    execute: {},
  },
  {
    name: "enumerable-setter-fails-closed",
    source: `
function Widget() {}
Object.defineProperty(Widget.prototype, "value", {
  enumerable: true,
  configurable: true,
  set: function() { use("set"); }
});
var descriptor = Object.getOwnPropertyDescriptor(Widget.prototype, "value");
use(descriptor.enumerable, descriptor.configurable);
var widget = new Widget();
widget.value = 1;
`,
    expected: ["Object.defineProperty(", "set ("],
    rejected: ["class "],
    execute: {},
  },
].map((snippet) => ({
  ...snippet,
  transformerFilter: ({ name }) => !name.startsWith("tsc-"),
}));

const snippets = [
  ...descriptorSnippets,
  {
    name: "typescript-accessor-version-boundary",
    source: `
class Widget {
  get value() { return this._value; }
  set value(next) { this._value = next; }
}
const first = new Widget();
const second = new Widget();
first.value = nextValue();
second.value = nextValue();
use(first.value, second.value);
`,
    expected: ["class Widget", "get value()", "set value(next)"],
    expectedAny: [
      ["class Widget", "get value()", "set value(next)"],
      ["class ", "get value()", "set value("],
    ],
    rejected: ["Object.defineProperty("],
    // TypeScript 3.5–3.8 intentionally emits enumerable:true here, while
    // 3.9+ emits the native class attributes. The standard-level recovery of
    // the older shape is tracked as a named source-recovery assumption; this
    // execution check observes accessor behavior but not descriptor metadata.
    execute: { returns: { nextValue: 7 } },
    transformerFilter: ({ name }) => name.startsWith("tsc-"),
  },
];

const allSources = snippets.map((snippet) => snippet.source);
const typescriptVersions = ["3.5.3", "3.8.3", "3.9.10", "4.3.5"];
const transformers = [
  ...withTerserVariants("source", allSources, (source) => source),
  {
    name: "swc-es5-minify-mangle",
    run: batchRunner(() => swcBatch(allSources, { target: "es5", minify: true })),
  },
  {
    name: "esbuild-es2015-minify-mangle",
    run: batchRunner(() => esbuildBatch(allSources, { target: "es2015", minify: true })),
  },
  ...typescriptVersions.flatMap((version) =>
    withTerserVariants(
      `tsc-${version}-es5`,
      allSources,
      batchRunner(() => tscBatch(allSources, { target: "ES5", version })),
    ),
  ),
];

runMatrix({
  name: "class-accessor",
  snippets,
  transformers,
  ...mangleValidator(),
});
