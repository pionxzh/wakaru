import transform from '../un-number-literal'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('transform number literal exponential notation',
  `
1e3
-2e4
`,
  `
1000
-20000
`,
)
