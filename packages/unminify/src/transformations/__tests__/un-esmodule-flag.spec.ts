import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-esmodule-flag'

const inlineTest = defineInlineTest(transform)

inlineTest('remove es module helper from ES5+',
  `
Object.defineProperty(exports, "__esModule", {
    value: true
});

const a = require('a');
`,
  `
const a = require('a');
`,
)

inlineTest('remove es module helper from ES3',
  `
exports.__esModule = true;
`,
  `
`,
)
