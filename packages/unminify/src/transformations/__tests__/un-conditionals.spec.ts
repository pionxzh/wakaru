import transform from '../un-conditionals'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('simple ternary expression #1',
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

inlineTest('simple ternary expression #2',
  `
obj[foo] = cond ? 10 : 20;
cond ? obj[bar] = 10 : obj[bar] = 20;
`,
  `
obj[foo] = cond ? 10 : 20;

if (cond) {
  obj[bar] = 10;
} else {
  obj[bar] = 20;
}
`,
)

inlineTest('simple logical expression',
  `
x && a();
x || b();
x ?? c();

!x && a();
!x || b();
!x ?? c();
`,
  `
if (x) {
  a();
}

if (!x) {
  b();
}

x ?? c();

if (!x) {
  a();
}

if (x) {
  b();
}

!x ?? c();
`,
)

inlineTest('nested ternary expression #1',
  `
a ? b() : c ? d() : e() ? g ? h() : i() : j()
`,
  `
if (a) {
  b();
} else if (c) {
  d();
} else if (e()) {
  if (g) {
    h();
  } else {
    i();
  }
} else {
  j();
}
`,
)

inlineTest('nested ternary expression #2',
  `
a ? b() : c ? d() : e() && (g || h());
`,
  `
if (a) {
  b();
} else if (c) {
  d();
} else if (e()) {
  if (!g) {
    h();
  }
}
`,
)

inlineTest('nested ternary expression #3',
  `
foo ? x() : bar ? y() : baz && z();
foo ? x() : bar ? y() : baz ? z() : t();
`,
  `
if (foo) {
  x();
} else if (bar) {
  y();
} else if (baz) {
  z();
}

if (foo) {
  x();
} else if (bar) {
  y();
} else if (baz) {
  z();
} else {
  t();
}
`,
)

inlineTest('nested ternary expression #4',
  `
a() && b() ? c() : d();
a() && b() && c();
`,
  `
if (a() && b()) {
  c();
} else {
  d();
}

if (a() && b()) {
  c();
}
`,
)

inlineTest('nested ternary expression #5',
  `
(foo && bar) ? x() : y();
(foo && bar) ? x() : (baz || t) ? y() : z();
`,
  `
if ((foo && bar)) {
  x();
} else {
  y();
}

if ((foo && bar)) {
  x();
} else if ((baz || t)) {
  y();
} else {
  z();
}
`,
)

inlineTest('return nested ternary expression #1',
  `
function fn () {
  return 2 == e ? foo() : 3 == f ? bar() : 4 == g ? baz() : fail(e)
}
`,
  `
function fn () {
  if (2 == e) {
    return foo();
  }

  if (3 == f) {
    return bar();
  }

  if (4 == g) {
    return baz();
  }

  return fail(e);
}
`,
)

inlineTest('return nested ternary expression #2',
  `
function fn () {
  return a ? b() : c ? d() : e()
}
`,
  `
function fn () {
  if (a) {
    return b();
  }

  if (c) {
    return d();
  }

  return e();
}
`,
)

inlineTest('nested logical expression',
  `
x == 'a' || x == 'b' || x == 'c' && finished()
`,
  `
if (x != 'a' && x != 'b') {
  if (x == 'c') {
    finished();
  }
}
`,
)

inlineTest('should not transform these cases',
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

inlineTest('switch statement #1',
`
foo == 'bar'
? bar()
: foo == 'baz'
  ? baz()
  : foo == 'qux'
    ? qux()
    : quux()
`,
`
switch (foo) {
case 'bar':
  bar();
  break;
case 'baz':
  baz();
  break;
case 'qux':
  qux();
  break;
default:
  quux();
  break;
}
`,
)

inlineTest('switch statement #2',
  `
foo == 'bar'
  ? bar()
  : foo == 'baz' || foo == 'baz2'
    ? baz()
    : foo == 'qux1' || foo == 'qux2' || foo == 'qux3'
      ? qux()
      : foo == 'quy4' || foo == 'quy5' || foo == 'quy6'
        ? quy()
        : quc()
`,
  `
switch (foo) {
case 'bar':
  bar();
  break;
case 'baz':
case 'baz2':
  baz();
  break;
case 'qux1':
case 'qux2':
case 'qux3':
  qux();
  break;
case 'quy4':
case 'quy5':
case 'quy6':
  quy();
  break;
default:
  quc();
  break;
}
`,
)

inlineTest('switch statement #3',
`
foo == 'bar'
  ? bar()
  : foo == 'baz'
    ? baz()
    : foo == 'qux' || foo == 'quux' && qux();
`,
`
switch (foo) {
case 'bar':
  bar();
  break;
case 'baz':
  baz();
  break;
case 'qux':
case 'quux':
  qux();
  break;
}
`,
)

inlineTest('switch statement #4',
  `
e === 2 || e === 9
  ? foo()
  : e === 3
  ? bar()
  : e === 4 || e === 5
  ? baz()
  : fail(e);
`,
`
switch (e) {
case 2:
case 9:
  foo();
  break;
case 3:
  bar();
  break;
case 4:
case 5:
  baz();
  break;
default:
  fail(e);
  break;
}
`,
)

inlineTest('return switch statement #1',
  `
function fn () {
  return foo == 'bar'
    ? bar()
    : foo == 'baz' || foo == 'baz2'
      ? baz()
      : foo == 'qux1' || foo == 'qux2' || foo == 'qux3'
        ? qux()
        : foo == 'quy4' || foo == 'quy5' || foo == 'quy6'
          ? quy()
          : quc()
}
`,
  `
function fn () {
  switch (foo) {
  case 'bar':
    return bar();
  case 'baz':
  case 'baz2':
    return baz();
  case 'qux1':
  case 'qux2':
  case 'qux3':
    return qux();
  case 'quy4':
  case 'quy5':
  case 'quy6':
    return quy();
  default:
    return quc();
  }
}
`,
)

inlineTest('should not transform these cases',
  `
x ? a : b;

x ? 1 : 2;

x && a || b;

x && 1;
`,
  `
x ? a : b;

x ? 1 : 2;

x && a || b;

x && 1;
`,
)
