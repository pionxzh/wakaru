import { defineInlineTest } from 'jscodeshift/src/testUtils'

import transform from '../un-variable-merging'

defineInlineTest(
    transform,
    {},
  `
var a= 1, b = true, c = "hello", d = 1.2, e = [1, 2, 3], f = {a: 1, b: 2, c: 3}, g = function() { return 1; }, h = () => 1;
`,
  `
var a= 1;
var b = true;
var c = "hello";
var d = 1.2;
var e = [1, 2, 3];
var f = {a: 1, b: 2, c: 3};
var g = function() { return 1; };
var h = () => 1;
`,
  'split sequence variable declaration',
)

defineInlineTest(
    transform,
    {},
  `
for (var i = 0, b = true, c = ''; i < 10; i++) {
    console.log(i);
}
`,
  `
for (var i = 0, b = true, c = ''; i < 10; i++) {
    console.log(i);
}
`,
  'for statement should not be transformed',
)
