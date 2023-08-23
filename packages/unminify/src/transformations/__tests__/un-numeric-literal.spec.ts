import transform from '../un-numeric-literal'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('transform number literal with different notation',
  `
65536;
123.4;
0b101010;
0o777;
-0x123;
4.2e2;
-2e4;
`,
  `
65536;
123.4;
42/* 0b101010 */;
511/* 0o777 */;
-291/* -0x123 */;
420/* 4.2e2 */;
-20000/* -2e4 */;
`,
)

inlineTest('transform number literal with comment',
  `
// comment
0b101010;
`,
  `
// comment
42/* 0b101010 */;
`,
)
