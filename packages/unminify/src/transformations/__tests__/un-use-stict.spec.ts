import { defineInlineTest } from '@unminify-kit/test-utils'
import transform from '../un-use-strict'

const inlineTest = defineInlineTest(transform)

inlineTest('remove \'use strict\'',
  `
'use strict'
`,
  `
`,
)

inlineTest('remove \'use strict\' with comments',
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
)
