import { defineInlineTest } from 'jscodeshift/src/testUtils'

import transform from '../un-use-strict'

defineInlineTest(
    transform,
    {},
  `
'use strict'
`,
  `
`,
  'remove \'use strict\'',
)

defineInlineTest(
    transform,
    {},
  `
// comment
// another comment
'use strict'
function foo(str) {
    'use strict'
    return str === 'use strict'
}
`,
  `
// comment
// another comment
function foo(str) {
    return str === 'use strict'
}
`,
  'remove \'use strict\' with comments',
)
