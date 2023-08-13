import transform from '../un-if-statement'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('nested ternary expression',
  `
a ? b() : c ? d() : e()
`,
  `
if (a) {
  b();
}

if (c) {
  d();
}

e();
`,
)

inlineTest('nested logical expression',
  `
x == 'a' || x == 'b' || x == 'c' && x == 'd'
`,
  `
x == 'a' || x == 'b' || x == 'c' && x == 'd'
`,
)

// inlineTest('return simple logical expression',
//   `
// return x == 'a' || x == 'b' || x == 'c' && x == 'd'
// `,
//   `
// if (!)
// `,
// )

inlineTest('simple ternary expression',
  `
x ? a() : b()
`,
  `
if (x) {
  a();
} else {
  b();
}
`,
)

inlineTest('simple logical expression',
  `
x && a();
x || b();
x ?? c();
`,
  `
if (x) {
  a();
}

if (!x) {
  b();
}

if (x == null) {
  c();
}
`,
)

inlineTest('should not transform if statement',
  `
var foo = x && a();

bar = x || a();

!(x && a());

if (x && a()) {
  b();
}

arr.push(x && a());

arr.push({ prop: x && a() });

function fn() {
  return x ? a() : b()
}

function fn2(p = x && a()) {
  return p && b();
}

for (var i = x && a(); i < 10; i++) {}

while (x && a()) {}

do {} while (x && a());
`,
  `
var foo = x && a();

bar = x || a();

!(x && a());

if (x && a()) {
  b();
}

arr.push(x && a());

arr.push({ prop: x && a() });

function fn() {
  return x ? a() : b()
}

function fn2(p = x && a()) {
  return p && b();
}

for (var i = x && a(); i < 10; i++) {}

while (x && a()) {}

do {} while (x && a());
`,
)

inlineTest('if-else statement with logical expression',
`
if (x) null === state && a();
else if (y) null !== state && b();
`,
`
if (x) {
  if (null === state) {
    a();
  }
} else if (y) {
  if (null !== state) {
    b();
  }
}
`,
)
