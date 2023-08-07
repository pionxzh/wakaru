import transform from '../un-number-literal'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('transform number literal exponential notation',
  `
65536
123.4
0b101010
0o777
-0x123
4.2e2
-2e4
`,
  `
65536
123.4
42
511
-291
420
-20000
`,
)
