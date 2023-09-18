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

while (a);

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

while (a)
  {}

do {
  b();
} while (a);

() => {
  return b();
};

label: b();
`,
)

inlineTest('should not blockify statements that has direct var declaration',
  `
'use strict';

if (a) var b = 1;
else if (c) var d = 1;
else var e = 1;

for (let i = 0; i < 10; i++) var b = 1;

for (let i in a) var b = 1;

for (let i of a) var b = 1;

while (a) var b = 1;

do var b = 1; while (a);
`,
  `
'use strict';

if (a) var b = 1;
else if (c) var d = 1;
else var e = 1;

for (let i = 0; i < 10; i++) var b = 1;

for (let i in a) var b = 1;

for (let i of a) var b = 1;

while (a) var b = 1;

do var b = 1; while (a);
`,
)
