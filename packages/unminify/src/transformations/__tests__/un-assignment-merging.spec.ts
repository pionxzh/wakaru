import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-assignment-merging'

const inlineTest = defineInlineTest(transform)

inlineTest('chained assignment should be splitted',
  `
exports.foo = exports.bar = exports.baz = 1;
`,
  `
exports.foo = 1;
exports.bar = 1;
exports.baz = 1;
`,
)

inlineTest('chained assignment should be splitted - allowed',
  `
a1 = a2 = 0;
b1 = b2 = 0n;
c1 = c2 = '';
d1 = d2 = true;
e1 = e2 = null;
f1 = f2 = undefined;
g1 = g2 = foo;
`,
  `
a1 = 0;
a2 = 0;
b1 = 0n;
b2 = 0n;
c1 = '';
c2 = '';
d1 = true;
d2 = true;
e1 = null;
e2 = null;
f1 = undefined;
f2 = undefined;
g1 = foo;
g2 = foo;
`,
)

inlineTest('chained assignment should be splitted - not allowed',
  `
a1 = a2 = \`template\${foo}\`;
b1 = b2 = Symbol();
c1 = c2 = /regex/;
d1 = d2 = foo.bar;
f1 = f2 = fn();
`,
  `
a1 = a2 = \`template\${foo}\`;
b1 = b2 = Symbol();
c1 = c2 = /regex/;
d1 = d2 = foo.bar;
f1 = f2 = fn();
`,
)

inlineTest('chained assignment should be splitted - comments',
  `
// before
exports.foo = exports.bar = exports.baz = 1;
// after
`,
  `
// before
exports.foo = 1;

exports.bar = 1;

exports.baz = 1;
// after
`,
)
