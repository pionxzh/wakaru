import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-boolean.grep'

const inlineTest = defineInlineTest(transform)

inlineTest('transform !0 to true and !1 to false',
  `
let a = !1;
const b = !0;

var obj = {
  value: !0
};
`,
  `
let a = false;
const b = true;

var obj = {
  value: true
};
`,
)
