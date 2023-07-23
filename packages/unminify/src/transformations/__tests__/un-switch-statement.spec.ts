import { defineInlineTest } from 'jscodeshift/src/testUtils'

import transform from '../un-switch-statement'

defineInlineTest(
    transform,
    {},
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
  {
    bar();
    break;
  }
case 'baz':
  {
    baz();
    break;
  }
case 'qux':
  {
    qux();
    break;
  }
default:
  {
    quux();
    break;
  }
};
`,
'should transform ternary to switch statement',
)

defineInlineTest(
    transform,
    {},
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
  {
    bar();
    break;
  }
case 'baz':
case 'baz2':
  {
    baz();
    break;
  }
case 'qux1':
case 'qux2':
case 'qux3':
  {
    qux();
    break;
  }
case 'quy4':
case 'quy5':
case 'quy6':
  {
    quy();
    break;
  }
default:
  {
    quc();
    break;
  }
};
`,
  'should transform ternary which contains multiple conditions to switch statement',
)

defineInlineTest(
    transform,
    {},
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
  {
    bar();
    break;
  }
case 'baz':
  {
    baz();
    break;
  }
case 'qux':
case 'quux':
  {
    qux();
    break;
  }
};
`,
'should transform ternary which contains multiple conditions to switch statement (no default)',
)
