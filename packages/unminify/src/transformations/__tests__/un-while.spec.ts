import transform from '../un-while'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('transform for(;;) to while(true)',
  `
for(;;) {
  console.log('hello')
}
`,
  `
while (true) {
  console.log('hello')
}
`,
)

inlineTest('should not transform for with init, test or update',
    `
for (let i = 0;;) {}

for (; i < 10;) {}

for (;; i++) {}
`,
    `
for (let i = 0;;) {}

for (; i < 10;) {}

for (;; i++) {}
`,
)
