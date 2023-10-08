import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-builtin-prototype'

const inlineTest = defineInlineTest(transform)

inlineTest('Convert function calls on instances of built-in objects to equivalent calls on their prototypes.',
  `
[].splice.apply(a, [1, 2, b, c]);
(function() {}).call.apply(console.log, console, ["foo"]),
(() => {}).call.apply(console.log,console,["foo"]);
0..toFixed.call(Math.PI, 2);
(0).toFixed.apply(Math.PI, [2]);
({}).hasOwnProperty.call(d, "foo");
/t/.test.call(/foo/, "bar");
/./.test.call(/foo/, "bar");
"".indexOf.call(e, "bar");
`,
    `
Array.prototype.splice.apply(a, [1, 2, b, c]);
Function.prototype.call.apply(console.log, console, ["foo"]),
Function.prototype.call.apply(console.log, console, ["foo"]);
Number.prototype.toFixed.call(Math.PI, 2);
Number.prototype.toFixed.apply(Math.PI, [2]);
Object.prototype.hasOwnProperty.call(d, "foo");
RegExp.prototype.test.call(/foo/, "bar");
RegExp.prototype.test.call(/foo/, "bar");
String.prototype.indexOf.call(e, "bar");
`,
)
