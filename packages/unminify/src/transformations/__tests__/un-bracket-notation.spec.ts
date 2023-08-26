import transform from '../un-bracket-notation'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('transform bracket notation to dot notation',
  `
obj['bar'];
obj['bar'].baz;
obj['bar']['baz'];
obj['bar'].baz['qux'];
`,
    `
obj.bar;
obj.bar.baz;
obj.bar.baz;
obj.bar.baz.qux;
`,
)

inlineTest('transform bracket notation with number',
    `
obj['1'];
obj['0'];
obj['00'];
obj['-0'];
obj['-1'];
`,
    `
obj[1];
obj[0];
obj['00'];
obj['-0'];
obj['-1'];
`,
)

inlineTest('remain bracket notation',
    `
obj[a];
obj[''];
obj[' '];
obj['var'];
obj['let'];
obj['const'];
obj['await'];
obj['1var'];
obj['prop-with-dash'];
`,
    `
obj[a];
obj[''];
obj[' '];
obj['var'];
obj['let'];
obj['const'];
obj['await'];
obj['1var'];
obj['prop-with-dash'];
`,
)
