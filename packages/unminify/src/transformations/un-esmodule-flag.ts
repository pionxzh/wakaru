import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Removes the `__esModule` flag from the module.
 *
 * @example
 * ```diff
 * - Object.defineProperty(exports, '__esModule', { value: true })
 * ```
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    /**
     * Target: ES5+
     * Object.defineProperty(exports, '__esModule', { value: true })
     */
    root
        .find(j.ExpressionStatement, {
            expression: {
                type: 'CallExpression',
                callee: {
                    type: 'MemberExpression',
                    object: { type: 'Identifier', name: 'Object' },
                    property: { type: 'Identifier', name: 'defineProperty' },
                },
                arguments: [
                    { type: 'Identifier', name: 'exports' } as const,
                    { type: 'Literal', value: '__esModule' } as const,
                ],
            },
        })
        .remove()

    /**
     * Target: ES3
     * exports.__esModule = true
     */
    root
        .find(j.AssignmentExpression, {
            left: {
                type: 'MemberExpression',
                object: { type: 'Identifier', name: 'exports' },
                property: { type: 'Identifier', name: '__esModule' },
            },
            operator: '=',
            right: { type: 'Literal', value: true },
        })
        .remove()
}

export default wrap(transformAST)
