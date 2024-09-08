import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import { defineInlineTest } from '@wakaru/test-utils'
import { convertExportGetter, convertExportsGetterForWebpack4, convertExportsGetterForWebpack5 } from '../src/extractors/webpack/requireHelpers'

const transformWebpack4 = createJSCodeshiftTransformationRule({
  name: 'test-webpack4-require-helpers',
  transform: (context) => {
    const { j, root } = context
    const collection = root.find(j.Program)
    const exportGetterMap = convertExportsGetterForWebpack4(j, collection)
    convertExportGetter(j, collection, true, exportGetterMap)
  },
})
const inlineTestWebpack4 = defineInlineTest(transformWebpack4)

const transformWebpack5 = createJSCodeshiftTransformationRule({
  name: 'test-webpack5-require-helpers',
  transform: (context) => {
    const { j, root } = context
    const collection = root.find(j.Program)
    const exportGetterMap = convertExportsGetterForWebpack5(j, collection)
    convertExportGetter(j, collection, true, exportGetterMap)
  },
})
const inlineTestWebpack5 = defineInlineTest(transformWebpack5)

inlineTestWebpack4('webpack 4 - require.d',
  `
require.d(exports, "a", function () { return a; });
require.d(exports, "b", () => c);
`,
  `
export { a };
export const b = c;
`,
)

inlineTestWebpack5('webpack5 - require.d',
  `
require.d(exports, {
  a: function () { return a; },
  b: () => bb,
  ['c']: function () { return cc; },
});
`,
  `
export { a };
export const b = bb;
export const c = cc;
`,
)
