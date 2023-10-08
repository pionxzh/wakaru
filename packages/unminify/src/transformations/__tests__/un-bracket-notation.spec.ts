import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-bracket-notation'

const inlineTest = defineInlineTest(transform)

inlineTest('transform bracket notation to dot notation',
  `
obj['bar'];
obj['bar'].baz;
obj['bar']['baz'];
obj['bar'].baz['qux'];

obj['\u0EB3']; // valid unicode
obj['\u001B']; // escape character
`,
    `
obj.bar;
obj.bar.baz;
obj.bar.baz;
obj.bar.baz.qux;

obj.ຳ; // valid unicode
obj['\u001B']; // escape character
`,
)

inlineTest('transform bracket notation with number',
    `
obj['1'];
obj['0'];
obj['00'];
obj['-0'];
obj['-1'];
obj['1_1'];

obj['3.14'];
obj['3.14e-10'];
obj['3.'];
obj['3..7'];
`,
    `
obj[1];
obj[0];
obj['00'];
obj['-0'];
obj['-1'];
obj['1_1'];

obj[3.14];
obj['3.14e-10'];
obj['3.'];
obj['3..7'];
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
