import { wrapAstTransformation } from '@wakaru/ast-utils'
import { defineInlineTest } from '@wakaru/test-utils'
import { convertExportGetter, convertExportsGetterForWebpack4, convertExportsGetterForWebpack5 } from '../src/extractors/webpack/requireHelpers'

const transformWebpack4 = wrapAstTransformation((context) => {
  const { j, root } = context
  const collection = root.find(j.Program)
  const exportGetterMap = convertExportsGetterForWebpack4(j, collection)
  convertExportGetter(j, collection, true, exportGetterMap)
})
const inlineTestWebpack4 = defineInlineTest(transformWebpack4)

const transformWebpack5 = wrapAstTransformation((context) => {
    const { j, root } = context
    const collection = root.find(j.Program)
    const exportGetterMap = convertExportsGetterForWebpack5(j, collection)
    convertExportGetter(j, collection, true, exportGetterMap)
})
const inlineTestWebpack5 = defineInlineTest(transformWebpack5)


inlineTestWebpack4('webpack 4 - require.d',
  `
require.d(exports, "a", function () { return a; });
require.d(exports, "b", () => b);
`,
  `
export const a = a;
export const b = b;
`,
)

inlineTestWebpack5('webpack5 - require.d',
  `
require.d(exports, {
  a: function () { return a; },
  b: () => b,
  ['c']: function () { return c; },
});
`,
  `
export const a = a;
export const b = b;
export const c = c;
`,
)
