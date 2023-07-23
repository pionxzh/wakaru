import { defineInlineTest } from 'jscodeshift/src/testUtils'

import transform from '../function-to-arrow'

defineInlineTest(
    transform,
    {},
  `
function add(a, b) { return a + b }
`,
  `
const add = (a, b) => { return a + b };
`,
  'transform function declaration to arrow function',
)
