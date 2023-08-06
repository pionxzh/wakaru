import { defineInlineTest } from 'jscodeshift/src/testUtils'

import transform from '../un-sequence-expression'

defineInlineTest(
    transform,
    {},
  `
a(), b(), c()
`,
  `
a();
b();
c();
`,
  'split sequence expression',
)

defineInlineTest(
    transform,
    {},
  `
return a(), b(), c()
`,
  `
a();
b();
return c();
`,
  'split return sequence expression',
)

defineInlineTest(
    transform,
    {},
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
'split if sequence expression',
)

defineInlineTest(
    transform,
    {},
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
'split while sequence expression',
)

defineInlineTest(
    transform,
    {},
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
'split do-while sequence expression',
)

defineInlineTest(
    transform,
    {},
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
'split switch sequence expression',
)

defineInlineTest(
    transform,
    {},
`
condition ? (a(), b()) : c()
`,
`
condition ? (a(), b()) : c()
`,
'do not split ternary sequence expression',
)

defineInlineTest(
    transform,
    {},
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
'split try catch sequence expression',
)

defineInlineTest(
    transform,
    {},
`
throw a(), b()
`,
`
a();
throw b();
`,
'split throw sequence expression',
)

defineInlineTest(
    transform,
    {},
`
const x = (a(), b(), c())
`,
`
a();
b();
const x = c();
`,
'split variable declaration sequence expression',
)

defineInlineTest(
    transform,
    {},
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
'split variable declaration sequence expression (advanced)',
)

defineInlineTest(
    transform,
    {},
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
'split for init sequence expression',
)

defineInlineTest(
    transform,
    {},
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
'split for init sequence expression (advanced)',
)
