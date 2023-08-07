import transform from '../un-export-rename'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('merge variable declaration and export declaration',
  `
const a = 1;
export const b = a;
`,
  `
export const b = 1;
`,
)

inlineTest('merge function declaration and export declaration',
    `
function a() {}
export const b = a;
`,
    `
export function b() {}
`,
)

inlineTest('merge class declaration and export declaration',
    `
class o {}
export const App = o
`,
    `
export class App {}
`,
)

inlineTest('merge class expression and export declaration',
    `
const o = class {};
export const App = o;
`,
    `
export const App = class {};
`,
)
