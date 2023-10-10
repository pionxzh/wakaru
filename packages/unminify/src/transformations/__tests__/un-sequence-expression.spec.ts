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

return a(), b(), c()
`,
  `
if (a) {
  b();
  return c();
}

a();
b();
return c();
`,
)

inlineTest('split if sequence expression',
`
if (a(), b(), c()) {
  d(), e()
}
`,
`
a();
b();

if (c()) {
  d();
  e();
}
`,
)

inlineTest('split while sequence expression',
`
while (a(), b(), c()) {
  d(), e()
}
`,
`
a();
b();

while (c()) {
  d();
  e();
}
`,
)

inlineTest('split do-while sequence expression',
`
do {
  d(), e()
} while (a(), b(), c())
`,
`
a();
b();

do {
  d();
  e();
} while (c());
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
`,
`
a();
b();

for (; c(); d(), e()) {
  f();
  g();
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
