import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-export-rename'

const inlineTest = defineInlineTest(transform)

inlineTest('merge variable declaration and export declaration',
  `
const a = 1;
console.log(a);
export const b = a, c = 2;
`,
  `
export const b = 1;
console.log(b);
export const c = 2;
`,
)

inlineTest('merge function declaration and export declaration',
  `
function a() {}
export const b = a;
`,
  `
export function b() {}
`,
)

inlineTest('merge function expression and export declaration with complex scope',
`
function test() {
    function a() {}
}
function a(n) {
    if (n < 2) return n;
    return a(n - 1) + a(n - 2);
}

export const fib = a;
`,
`
function test() {
    function a() {}
}

export function fib(n) {
    if (n < 2) return n;
    return fib(n - 1) + fib(n - 2);
}
`)

inlineTest('merge arrow function expression and export declaration',
  `
const a = () => {}
export const b = a
`,
  `
export const b = () => {};
`,
)

inlineTest('merge class declaration and export declaration',
  `
class o {}
export const App = o
`,
  `
export class App {}
`,
)

inlineTest('merge class expression and export declaration',
  `
const o = class {};
export const App = o;
`,
  `
export const App = class {};
`,
)

inlineTest('do not modify redeclare export declaration when the newName is declared',
  `
const a = 1;
const b = 2;
export { b as a };
`,
  `
const a = 1;
const b = 2;
export { b as a };
`,
)

inlineTest('do not modify export default',
  `
const o = class {};
export default o;
`,
  `
const o = class {};
export default o;
`,
)

// FIXME: https://github.com/facebook/jscodeshift/issues/263
// JSCodeShift and `ast-types` didn't create the correct scope for BlockStatement
inlineTest.todo('should handle the scope correctly',
  `
const a = 1;
console.log(a);
{
    const a = 2;
    console.log(a);
}
function test() {
    const a = 3;
    console.log(a);
}
for(let a = 4; a < 5; a++) {
    console.log(a);
}
export const b = a, c = 2;
`,
  `
export const b = 1;
console.log(b);
{
    const a = 2;
    console.log(a);
}
function test() {
    const a = 3;
    console.log(a);
}
for(let a = 4; a < 5; a++) {
    console.log(a);
}
export const c = 2;
`,
)
