import { defineInlineTest } from '@unminify-kit/test-utils'
import transform from '../un-type-constructor'

const inlineTest = defineInlineTest(transform)

inlineTest('Restore type constructors from minified code.',
  `
+x;
x + "";
[,,,];
`,
    `
Number(x);
String(x);
Array(3);
`,
)

inlineTest('complex cases',
    `
var a = 6 + +x;
var b = x + "a";
var c = 'long string' + x + '';
var d = x + 5 + '';
var e = x + '' + 5;
var f = 'str' + x + '' + 5 + '' + 6;
var g = 'str' + '';

function foo(numStr, result) {
    var num = +numStr;
    var arr = [,,,].fill(num + '').join(' + ');
    return \`\${result} = \${arr}\`;
}

`,
    `
var a = 6 + Number(x);
var b = x + "a";
var c = String('long string' + x);
var d = String(x + 5);
var e = String(x) + 5;
var f = String(String('str' + x) + 5) + 6;
var g = 'str';

function foo(numStr, result) {
    var num = Number(numStr);
    var arr = Array(3).fill(String(num)).join(' + ');
    return \`\${result} = \${arr}\`;
}
`,
)
