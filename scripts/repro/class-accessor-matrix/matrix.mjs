#!/usr/bin/env node

import {
  batchRunner,
  esbuildBatch,
  runMatrix,
  swcBatch,
  withTerserVariants,
} from "../lib/runner.mjs";
import { mangleValidator } from "../lib/compare.mjs";

// Class syntax cannot express a zero-argument setter, but descriptor callbacks
// can and do occur in real bundles. These inputs start at that legal ES5 shape
// and exercise the distinct class-recovery paths after production minifiers.
const snippets = [
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
];

const allSources = snippets.map((snippet) => snippet.source);
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
];

runMatrix({
  name: "class-accessor",
  snippets,
  transformers,
  ...mangleValidator(),
});
