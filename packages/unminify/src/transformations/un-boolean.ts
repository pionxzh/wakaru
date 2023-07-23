import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * !0 -> true
 * !1 -> false
 *
 * @see https://babeljs.io/docs/en/babel-plugin-transform-minify-booleans
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.UnaryExpression, {
            operator: '!',
            argument: { type: 'Literal' },
        })
        .forEach((p) => {
            if (!j.Literal.check(p.node.argument)) return

            const { value } = p.node.argument
            const is01 = value === 0 || value === 1
            if (!is01) return
            p.replace(j.booleanLiteral(!value))
        })
}

export default wrap(transformAST)
