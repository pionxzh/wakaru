import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import type { ASTTransformation } from '@wakaru/shared/rule'
import type { NumericLiteral } from 'jscodeshift'

/**
 * Converts minified `boolean` to simple `true`/`false`.
 *
 * @example
 * !0 -> true
 * !1 -> false
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-minify-booleans
 * @see Terser: `booleans_as_integers`
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.UnaryExpression, {
            operator: '!',
            argument: { type: 'NumericLiteral' },
        })
        .forEach((p) => {
            const { value } = p.node.argument as NumericLiteral
            const is01 = value === 0 || value === 1
            if (!is01) return
            p.replace(j.booleanLiteral(!value))
        })
}

export default createJSCodeshiftTransformationRule({
    name: 'un-boolean',
    transform: transformAST,
})
