import { defineInlineTest } from 'jscodeshift/src/testUtils'

import transform from '../un-boolean'

defineInlineTest(
    transform,
    {},
  `
let a = !1
const b = !0

var obj = {
  value: !0
};
`,
  `
let a = false
const b = true

var obj = {
  value: true
};
`,
  'transform !0 to true and !1 to false',
)
