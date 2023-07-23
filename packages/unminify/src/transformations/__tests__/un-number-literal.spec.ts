import { defineInlineTest } from 'jscodeshift/src/testUtils'

import transform from '../un-number-literal'

defineInlineTest(
    transform,
    {},
  `
1e3
-2e4
`,
  `
1000
-20000
`,
  'transform number literal exponential notation',
)
