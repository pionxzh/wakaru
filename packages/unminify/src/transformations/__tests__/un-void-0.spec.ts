import transform from '../un-void-0'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('transform void 0 to undefined',
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
)
