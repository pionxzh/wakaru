import transform from '../un-bracket-notation'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('transform bracket notation to dot notation',
  `
obj['bar'];
obj['1'];
`,
    `
obj.bar;
obj[1];
`,
)

inlineTest('remain bracket notation',
    `
obj[a];
obj['var'];
obj['let'];
obj['const'];
obj['await'];
obj['00'];
obj['1var'];
`,
    `
obj[a];
obj['var'];
obj['let'];
obj['const'];
obj['await'];
obj['00'];
obj['1var'];
`,
)
