import { defineInlineTest } from 'jscodeshift/src/testUtils'

import transform from '../un-void-0'

defineInlineTest(
    transform,
    {},
  `
if(void 0 !== a) {
  console.log('a')
}
`,
  `
if(undefined !== a) {
  console.log('a')
}
`,
  'transform void 0 to undefined',
)
