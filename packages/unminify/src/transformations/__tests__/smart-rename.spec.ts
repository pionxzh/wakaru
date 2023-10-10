import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../smart-rename'

const inlineTest = defineInlineTest(transform)

inlineTest('object destructuring rename',
  `
const {
  gql: t,
  dispatchers: o,
  listener: i
} = n;
o.delete(t, i);
`,
  `
const {
  gql,
  dispatchers,
  listener
} = n;
dispatchers.delete(gql, listener);
`,
)
