import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { Literal } from 'jscodeshift'

/**
 * Transform minified `boolean` to their simpler forms.
 *
 * @example
 * !0 -> true
 * !1 -> false
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-minify-booleans
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.UnaryExpression, {
            operator: '!',
            argument: { type: 'Literal' },
        })
        .forEach((p) => {
            const { value } = p.node.argument as Literal
            const is01 = value === 0 || value === 1
            if (!is01) return
            p.replace(j.booleanLiteral(!value))
        })
}

export default wrap(transformAST)
