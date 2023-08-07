import transform from '../un-es-helper'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('remove es module helper',
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
