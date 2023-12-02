import { isExportObject, isLooseTrue, isStringObjectProperty } from '@wakaru/ast-utils/matchers'
import { wrapAstTransformation } from '@wakaru/ast-utils/wrapAstTransformation'
import type { ASTTransformation } from '@wakaru/ast-utils/wrapAstTransformation'
import type { CallExpression, MemberExpression } from 'jscodeshift'

/**
 * Removes the `__esModule` flag from the module.
 *
 * @example
 * ```diff
 * - Object.defineProperty(exports, '__esModule', { value: true })
 * - exports.__esModule = !0
 * - module.exports.__esModule = true
 * ```
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    /**
     * Target: ES5+
     * Object.defineProperty(exports, '__esModule', { value: true })
     * Object.defineProperty(module.exports, '__esModule', { value: true })
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
                    (node: CallExpression['arguments'][0]) => isExportObject(j, node),
                    { type: 'StringLiteral', value: '__esModule' } as const,
                ],
            },
        })
        .remove()

    /**
     * Target: ES3
     * exports.__esModule = true
     * module.exports.__esModule = true
     */
    root
        .find(j.AssignmentExpression, {
            left: {
                type: 'MemberExpression',
                object: (node: MemberExpression['object']) => isExportObject(j, node),
                property: (node: MemberExpression['property']) => isStringObjectProperty(j, node, '__esModule'),
            },
            operator: '=',
            right: node => isLooseTrue(j, node),
        })
        .remove()
}

export default wrapAstTransformation(transformAST)
