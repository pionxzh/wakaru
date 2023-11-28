import { wrapAstTransformation } from '@wakaru/ast-utils'
import type { ASTTransformation } from '@wakaru/ast-utils'

/**
 * Converts `1 / 0` to `Infinity`.
 *
 * @example
 * `1 / 0` -> `Infinity`
 *
 * @see https://babeljs.io/docs/babel-plugin-minify-infinity
 * @see Terser: `keep_infinity`
 * @see https://github.com/terser/terser/blob/931f8a5fd548795faae0da1fa9eafa3f2ad1647b/lib/compress/index.js#L2641
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    /**
     * Logically, 99 / 0 is Infinity, but it's not necessary to transform it.
     * It's not a common pattern to write 99 / 0, and we don't want to
     * waste time on it.
     */

    root
        .find(j.BinaryExpression, {
            operator: '/',
            left: { type: 'NumericLiteral', value: 1 },
            right: { type: 'NumericLiteral', value: 0 },
        })
        .forEach((p) => {
            p.replace(j.identifier('Infinity'))
        })

    root
        .find(j.BinaryExpression, {
            operator: '/',
            left: { type: 'UnaryExpression', operator: '-', argument: { type: 'NumericLiteral', value: 1 } },
            right: { type: 'NumericLiteral', value: 0 },
        })
        .forEach((p) => {
            p.replace(j.unaryExpression('-', j.identifier('Infinity')))
        })
}

export default wrapAstTransformation(transformAST)
