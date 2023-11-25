import { isLooseTrue } from '../utils/checker'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { ASTNode, CallExpression, Identifier, JSCodeshift, MemberExpression, StringLiteral } from 'jscodeshift'

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
                property: (node: MemberExpression['property']) => is__esModule(j, node),
            },
            operator: '=',
            right: node => isLooseTrue(j, node),
        })
        .remove()
}

function isExportObject(j: JSCodeshift, node: ASTNode): node is MemberExpression | Identifier {
    return isExports(j, node) || isModuleExports(j, node)
}

function isExports(j: JSCodeshift, node: ASTNode): node is Identifier {
    return j.Identifier.check(node) && node.name === 'exports'
}

function isModuleExports(j: JSCodeshift, node: ASTNode): node is MemberExpression {
    return j.MemberExpression.check(node)
        && j.Identifier.check(node.object) && node.object.name === 'module'
        && j.Identifier.check(node.property) && node.property.name === 'exports'
}

const __esModule = '__esModule'
function is__esModule(j: JSCodeshift, node: ASTNode): node is Identifier | StringLiteral {
    return (j.Identifier.check(node) && node.name === __esModule)
        || (j.StringLiteral.check(node) && node.value === __esModule)
}

export default wrap(transformAST)
