import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Transform `void 0` to `undefined`.
 *
 * @example
 * void 0 -> undefined
 * void 99 -> undefined
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-undefined-to-void
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.UnaryExpression, {
            operator: 'void',
            argument: { type: 'Literal' },
        })
        .forEach((p) => {
            if (!j.Literal.check(p.node.argument)) return

            if (j.Literal.check(p.node.argument)) {
                p.replace(j.identifier('undefined'))
            }
        })
}

export default wrap(transformAST)
