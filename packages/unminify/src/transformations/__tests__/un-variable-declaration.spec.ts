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
  'variable declaration should be splitted',
)

defineInlineTest(transform,
    {},
    `
var a = 1, b = 2, c = 3;

let d = 1, e = 2, f = 3;

const g = 1, h = 2, i = 3;
`,
  `
var a = 1;
var b = 2;
var c = 3;
let d = 1;
let e = 2;
let f = 3;
const g = 1;
const h = 2;
const i = 3;
`,
  'variable declaration should be splitted with the original type',
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
  'variable declaration in for statement should not be transformed',
)
