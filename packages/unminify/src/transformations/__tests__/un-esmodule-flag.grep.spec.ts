import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-esmodule-flag.grep'

const inlineTest = defineInlineTest(transform)

inlineTest('remove es module helper from ES5+',
  `
Object.defineProperty(exports, "__esModule", {
  value: true
});
Object.defineProperty(module.exports, "__esModule", {
  value: !0
});
`,
  `
`,
)

inlineTest('remove es module helper from ES3',
  `
exports.__esModule = !0;
exports.__esModule = true;
exports["__esModule"] = true;
module.exports.__esModule = !0;
module.exports.__esModule = true;
module.exports["__esModule"] = true;
`,
  `
`,
)
