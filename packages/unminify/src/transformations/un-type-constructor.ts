import { isString } from '@wakaru/ast-utils'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Restore type constructors from minified code.
 *
 * @example
 * +x; // Number(x)
 * x + ""; // String(x)
 * [,,,]; // Array(3)
 *
 * @example
 * // We don't transform the following cases because the original code is more readable.
 *
 * !!x; // Boolean(x)
 * [3, 1]; // Array.of(3, 1)
 * {foo: 'bar'}; // Object({foo: 'bar'})
 *
 * @see https://babeljs.io/docs/babel-plugin-minify-type-constructors
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    /**
     * +x -> Number(x)
     *
     * Unsafe Warning:
     * 1. BigInt
     *   - +1n // throw TypeError
     */
    root.find(j.UnaryExpression, { operator: '+', argument: { type: 'Identifier' } }).replaceWith(({ node }) => {
        return j.callExpression(j.identifier('Number'), [node.argument])
    })

    /**
     * x + '' -> String(x)
     *
     * Unsafe Warning:
     * 1. Multiple concatenations.
     *   - This is more like a reminder for developers.
     *   - Our current implementation should not fail in this case.
     *   - var x = 5; x + 5 + '' // '10'
     *   - var x = 5; x + '' + 5 // '55'
     * 2. Symbol
     *   - Symbol('foo') + '' // throw TypeError
     */
    root.find(j.BinaryExpression, {
        operator: '+',
        right: { type: 'Literal', value: '' },
    }).forEach((path) => {
        // 'str' + '' will be simplified to 'str'
        if (j.Literal.check(path.node.left) && isString(path.node.left.value)) {
            path.replace(path.node.left)
            return
        }

        path.replace(j.callExpression(j.identifier('String'), [path.node.left]))
    })

    /**
     * [,,,] -> Array(3)
     */
    root
        .find(j.ArrayExpression, {
            elements: (elements) => {
                return elements.length > 0
                && elements.every(element => element === null)
            },
        })
        .replaceWith(({ node }) => {
            return j.callExpression(j.identifier('Array'), [j.literal(node.elements.length)])
        })
}

export default wrap(transformAST)
