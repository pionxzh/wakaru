import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-import-rename'

const inlineTest = defineInlineTest(transform)

inlineTest('import rename',
  `
import { foo as a, bar as b, code } from '_';

console.log(a, b, code);
`,
  `
import { foo, bar, code } from '_';

console.log(foo, bar, code);
`,
)

inlineTest('import rename with naming conflict',
  `
import defaultExport, { foo as a, bar as b, code } from 'A';
import { foo, bar as c } from 'B';

console.log(a, b, code, foo, c);
`,
  `
import defaultExport, { foo as foo_1, bar, code } from 'A';
import { foo, bar as bar_1 } from 'B';

console.log(foo_1, bar, code, foo, bar_1);
`,
)

inlineTest('multiple naming conflicts',
  `
import { foo as a, bar as b } from 'A';
import { foo, bar } from 'B';

const foo_1 = 'local';
console.log(a, b, foo, bar, foo_1);
`,
  `
import { foo as foo_2, bar as bar_1 } from 'A';
import { foo, bar } from 'B';

const foo_1 = 'local';
console.log(foo_2, bar_1, foo, bar, foo_1);
`,
)
