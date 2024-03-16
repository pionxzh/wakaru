import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-sequence-expression'

const inlineTest = defineInlineTest(transform)

inlineTest('split sequence expression',
  `
a(), b(), c()
`,
  `
a();
b();
c();
`,
)

inlineTest('split sequence expression - comments',
  `
// leading comment
a(), b(), c()
// trailing comment
`,
  `
// leading comment
a();

b();

c();
// trailing comment
`,
)

inlineTest('split return sequence expression',
  `
if(a) return b(), c();
else return d = 1, e = 2, f = 3;

return a(), b(), c()
`,
  `
if (a) {
  b();
  return c();
} else {
  d = 1;
  e = 2;
  f = 3;
  return f;
}

a();
b();
return c();
`,
)

inlineTest('split sequence expression in arrow function body',
  `
var foo = (m => (a(), b(), c))();
var bar = (m => (m.a = 1, m.b = 2, m.c = 3))();
`,
  `
var foo = (m => {
  a();
  b();
  return c;
})();
var bar = (m => {
  m.a = 1;
  m.b = 2;
  m.c = 3;
  return m.c;
})();
`,
)

inlineTest('split if sequence expression',
  `
if (condition) a(), b();
else c(), d();

if (a(), b(), c()) {
  d(), e()
}
`,
  `
if (condition) {
  a();
  b();
} else {
  c();
  d();
}

a();
b();

if (c()) {
  d();
  e();
}
`,
)

inlineTest('do not split while sequence expression',
  `
while (a(), b(), c()) {
  d(), e()
}
`,
  `
while (a(), b(), c()) {
  d();
  e();
}
`,
)

inlineTest('do not split do-while sequence expression',
  `
do {
  d(), e()
} while (a(), b(), c())
`,
  `
do {
  d();
  e();
} while (a(), b(), c())
`,
)

inlineTest('split switch sequence expression',
  `
switch (a(), b(), c()) {
  case 1:
    d(), e()
}
`,
  `
a();
b();

switch (c()) {
case 1:
  d();
  e();
}
`,
)

inlineTest('do not split ternary sequence expression',
  `
condition ? (a(), b()) : c()
`,
  `
condition ? (a(), b()) : c()
`,
)

inlineTest('split try catch sequence expression',
  `
try {
  a(), b()
} catch (e) {
  c(), d()
}
`,
  `
try {
  a();
  b();
} catch (e) {
  c();
  d();
}
`,
)

inlineTest('split throw sequence expression',
  `
if(e !== null) throw a(), e
`,
  `
if (e !== null) {
  a();
  throw e;
}
`,
)

inlineTest('split variable declaration sequence expression',
  `
const x = (a(), b(), c())
`,
  `
a();
b();
const x = c();
`,
)

inlineTest('split variable declaration sequence expression (advanced)',
  `
const x = (a(), b(), c()), y = 3, z = (d(), e())
`,
  `
a();
b();
const x = c();
const y = 3;
d();
const z = e();
`,
)

inlineTest('split for init sequence expression',
  `
for (a(), b(); c(); d(), e()) {
  f(), g()
}

var o = [];
for (var x in o.push("PASS"), o) {
  console.log(o[x]);
}

for (let x in (a(), b(), c())) {
  console.log(x);
}
`,
  `
a();
b();

for (; c(); d(), e()) {
  f();
  g();
}

var o = [];
o.push("PASS");

for (var x in o) {
  console.log(o[x]);
}

a();
b();

for (let x in c()) {
  console.log(x);
}
`,
)

inlineTest('split for init sequence expression (advanced)',
  `
for (let x = (a(), b(), c()), y = 1; x < 10; x++) {
  d(), e()
}
`,
  `
a();
b();

for (let x = c(), y = 1; x < 10; x++) {
  d();
  e();
}
`,
)

inlineTest('split member expression in assignment',
  `
(a = b())['c'] = d;
// comment
(a = v).b = c;
`,
  `
a = b();
a['c'] = d;

// comment
a = v;

a.b = c;
`,
)
