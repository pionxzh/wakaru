import { defineInlineTest } from 'jscodeshift/src/testUtils'

import transform from '../un-es-helper'

defineInlineTest(
    transform,
    {},
  `
Object.defineProperty(exports, "__esModule", {
    value: true
});

const a = require('a');
`,
  `
const a = require('a');
`,
  'remove es module helper',
)
