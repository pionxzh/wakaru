import { defineInlineTest } from 'jscodeshift/src/testUtils'

import transform from '../un-flip-comparisons'

defineInlineTest(
    transform,
    {},
  `
const a = undefined === foo
const b = null !== foo
const c = 1 == foo
const d = "str" != foo
const e = "function" != typeof foo

const f = 1 < bar
const g = 1 > bar
const h = 1 <= bar
const i = 1 >= bar
`,
  `
const a = foo === undefined
const b = foo !== null
const c = foo == 1
const d = foo != "str"
const e = typeof foo != "function"

const f = bar > 1
const g = bar < 1
const h = bar >= 1
const i = bar <= 1
`,
  'flip comparisons back',
)
