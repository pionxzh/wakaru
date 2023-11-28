import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-undefined.grep'

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

inlineTest.fixme('should not transform when undefined is declared in scope',
  `
var undefined = 42;

console.log(void 0);

if (undefined !== a) {
  console.log('a', void 0);
}
`,
  `
var undefined = 42;

console.log(void 0);

if (undefined !== a) {
  console.log('a', void 0);
}
`,
)
