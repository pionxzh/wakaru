import { isDeclared } from '../utils/scope'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Converts `void 0` to `undefined`.
 *
 * @example
 * void 0 -> undefined
 * void 99 -> undefined
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-undefined-to-void
 * @see Terser: `unsafe_undefined`
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.UnaryExpression, {
            operator: 'void',
            argument: { type: 'Literal' },
        })
        .forEach((p) => {
            if (isDeclared(p.scope, 'undefined')) return

            if (j.Literal.check(p.node.argument)) {
                p.replace(j.identifier('undefined'))
            }
        })
}

export default wrap(transformAST)
