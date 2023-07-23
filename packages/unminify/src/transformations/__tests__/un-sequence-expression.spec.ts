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
