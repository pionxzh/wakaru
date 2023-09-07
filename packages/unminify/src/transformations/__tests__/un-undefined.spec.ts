import { defineInlineTest } from '@unminify-kit/test-utils'
import transform from '../un-undefined'

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

inlineTest('transform void literal to undefined',
  `
void 0
void 99
void(0)
`,
  `
undefined
undefined
undefined
`,
)

inlineTest('should not transform void function call',
  `
void function() {
  console.log('a')
  return void a()
}
`,
  `
void function() {
  console.log('a')
  return void a()
}
`,
)
