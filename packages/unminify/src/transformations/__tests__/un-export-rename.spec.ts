import { defineInlineTest } from 'jscodeshift/src/testUtils'

import transform from '../un-export-rename'

defineInlineTest(
    transform,
    {},
  `
const a = 1;
export const b = a;
`,
  `
export const b = 1;
`,
  'merge variable declaration and export declaration',
)

defineInlineTest(
    transform,
    {},
    `
function a() {}
export const b = a;
`,
    `
export function b() {}
`,
    'merge function declaration and export declaration',
)

defineInlineTest(
    transform,
    {},
    `
class o {}
export const App = o
`,
    `
export class App {}
`,
    'merge class declaration and export declaration',
)

defineInlineTest(
    transform,
    {},
    `
const o = class {};
export const App = o;
`,
    `
export const App = class {};
`,
    'merge class expression and export declaration',
)
