import { defineInlineTest } from '@unminify-kit/test-utils'
import transform from '../un-curly-braces'

const inlineTest = defineInlineTest(transform)

inlineTest('blockify statements',
  `
'use strict';

if (a) b();

if (a) b();
else if (c) d();
else e();

for (let i = 0; i < 10; i++) b();

for (let i in a) b();

for (let i of a) b();

while (a) b();

do
  b();
while (a);

() => b();

label: b();
`,
  `
'use strict';

if (a) {
  b();
}

if (a) {
  b();
} else if (c) {
  d();
} else {
  e();
}

for (let i = 0; i < 10; i++) {
  b();
}

for (let i in a) {
  b();
}

for (let i of a) {
  b();
}

while (a) {
  b();
}

do {
  b();
} while (a);

() => {
  return b();
};

label:
{
  b();
}
`,
)
