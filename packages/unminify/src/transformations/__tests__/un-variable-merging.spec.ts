import { defineInlineTest } from '@unminify-kit/test-utils'
import transform from '../un-variable-merging'

const inlineTest = defineInlineTest(transform)

inlineTest('variable declaration should be splitted',
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
)

inlineTest('variable declaration should be splitted with the original type',
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
)

inlineTest('variable declaration that is not used in for statement should not be splitted',
  `
for (var i = 0, j = 0, k = 0; j < 10; k++) {
  console.log(k);
}
`,
  `
var i = 0;
for (var j = 0, k = 0; j < 10; k++) {
  console.log(k);
}
`,
)

inlineTest('variable declaration with kind other than var should not be splitted',
  `
for (let i = 0, j = 0, k = 0; j < 10; k++) {}
for (const i = 0, j = 0, k = 0; j < 10; k++) {}
`,
  `
for (let i = 0, j = 0, k = 0; j < 10; k++) {}
for (const i = 0, j = 0, k = 0; j < 10; k++) {}
`,
)

inlineTest('should prune empty variable declaration in for statement',
  `
for (var i = 0; j < 10; k++) {}
`,
  `
var i = 0;
for (; j < 10; k++)
  {}
`,
)

inlineTest('should not split if there is a same variable declaration in parent scope',
  `
var i = 99;
for (var i = 0, j = 0, k = 0; j < 10; j++) {}
`,
  `
var i = 99;
var k = 0;
for (var i = 0, j = 0; j < 10; j++) {}
`,
)
