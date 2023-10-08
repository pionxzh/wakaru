import { defineInlineTest } from '@wakaru/test-utils'
import { renameFunctionParameters } from '../src/renameFunctionParameters'
import { wrapAstTransformation } from '../src/wrapAstTransformation'

const transform = wrapAstTransformation((context) => {
    const { root, j } = context

    root
        .find(j.FunctionDeclaration)
        .forEach((path) => {
            const node = path.node
            renameFunctionParameters(j, node, ['c', 'd'])
        })
})

const inlineTest = defineInlineTest(transform)

inlineTest('should rename function parameters',
  `
function foo(a, b) {
  const obj = {
    a: a.a,
    b: b.c,
    c: e.b,
  }

  return a + b;
}
`,
  `
function foo(c, d) {
  const obj = {
    a: c.a,
    b: d.c,
    c: e.b,
  }

  return c + d;
}
`,
)
