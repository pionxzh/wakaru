import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-infinity'

const inlineTest = defineInlineTest(transform)

inlineTest('transform {number} / 0 to Infinity',
  `
0 / 0;
1 / 0;
-1 / 0;
99 / 0;

'0' / 0;
'1' / 0;
'-1' / 0;
'99' / 0;

x / 0;

[0 / 0, 1 / 0]
`,
  `
0 / 0;
Infinity;
-Infinity;
99 / 0;

'0' / 0;
'1' / 0;
'-1' / 0;
'99' / 0;

x / 0;

[0 / 0, Infinity]
`,
)
