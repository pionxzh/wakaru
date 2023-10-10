import { findReferences } from '@wakaru/ast-utils'
import { findHelperLocals, removeHelperImport } from '../../../utils/import'
import { markParenthesized } from '../../../utils/parenthesized'
import wrap from '../../../wrapAstTransformation'
import type { SharedParams } from '../../../utils/types'
import type { ASTTransformation } from '../../../wrapAstTransformation'
import type { Scope } from 'ast-types/lib/scope'
import type { ASTPath, AssignmentExpression, CallExpression, Identifier, MemberExpression, NumericLiteral, SequenceExpression, VariableDeclarator } from 'jscodeshift'

/**
 * Restores default import from `@babel/runtime/helpers/interopRequireDefault` helper.
 * This operation is *not* safe, because it assumes that the default export is always present,
 * and the transformed require will soon be transformed into an import.
 *
 * ```ts
 * function interopRequireDefault(obj)
 * ```
 *
 * @example
 * var _a = interopRequireDefault(require("a"));
 * var _b = require("b");
 * (0, _a.default)();
 * ->
 * var _a = require("a");
 * var _b = require("b");
 * a();
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-modules-commonjs
 */
export const transformAST: ASTTransformation<SharedParams> = (context, params) => {
    const moduleName = '@babel/runtime/helpers/interopRequireDefault'
    const moduleEsmName = '@babel/runtime/helpers/esm/interopRequireDefault'

    const { root, j } = context
    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    const helperLocals = findHelperLocals(context, params, moduleName, moduleEsmName)
    helperLocals.forEach((helperLocal) => {
        let processed = 0

        const references = findReferences(j, rootScope, helperLocal)

        references
            .filter((path) => {
                const parentNode = path.parent?.node
                if (!parentNode) return false

                return j.CallExpression.check(parentNode)
                    && parentNode.callee === path.node
                    && parentNode.arguments.length === 1
            })
            .forEach((path) => {
                const callExpression = path.parent?.node as CallExpression
                const arg = callExpression.arguments[0]
                if (j.SpreadElement.check(arg)) return

                // var _a = interopRequireDefault(require("a")).default;
                if (j.MemberExpression.check(path.parent?.parent?.node)) {
                    const memberExpression = path.parent.parent.node as MemberExpression
                    if (
                        j.Identifier.check(memberExpression.property)
                        && memberExpression.property.name === 'default'
                    ) {
                        path.parent.parent.replace(arg)

                        // early return because the `default` property is already handled
                        // no more transformations are needed
                        processed += 1
                        return
                    }
                }

                if (
                    j.VariableDeclarator.check(path.parent?.parent?.node)
                 || j.AssignmentExpression.check(path.parent?.parent?.node)
                ) {
                    const declarator = path.parent.parent.node as VariableDeclarator | AssignmentExpression
                    let id: Identifier | null = null
                    if (j.VariableDeclarator.check(declarator) && j.Identifier.check(declarator.id)) {
                        id = declarator.id
                    }
                    else if (j.AssignmentExpression.check(declarator) && j.Identifier.check(declarator.left)) {
                        id = declarator.left
                    }

                    if (id) {
                        const idReferences = findReferences(j, rootScope, id.name)
                        idReferences.forEach((idReference) => {
                            // (0, id.default)(...) -> id(...)
                            const seq = idReference.parent?.parent as ASTPath<SequenceExpression>
                            if (
                                j.SequenceExpression.check(seq?.node)
                                && seq.node.expressions.length === 2
                                && j.NumericLiteral.check(seq.node.expressions[0])
                                && (seq.node.expressions[0] as NumericLiteral).value === 0
                                && j.MemberExpression.check(seq.node.expressions[1])
                                && seq.node.expressions[1].object === idReference.node
                                && j.Identifier.check(seq.node.expressions[1].property)
                                && seq.node.expressions[1].property.name === 'default'
                                && j.CallExpression.check(seq.parent?.node)
                            ) {
                                seq.replace(idReference.node)
                                markParenthesized(seq.parent.node, false)
                            }

                            // id.default -> id
                            if (
                                j.MemberExpression.check(idReference.parent?.node)
                                && idReference.parent.node.object === idReference.node
                                && j.Identifier.check(idReference.parent.node.property)
                                && idReference.parent.node.property.name === 'default'
                            ) {
                                idReference.parent.replace(idReference.node)
                            }

                            // id['default'] -> id
                            if (
                                j.MemberExpression.check(idReference.parent?.node)
                                && idReference.parent.node.object === idReference.node
                                && j.StringLiteral.check(idReference.parent.node.property)
                                && idReference.parent.node.property.value === 'default'
                            ) {
                                idReference.parent.replace(idReference.node)
                            }
                        })
                    }
                }

                path.parent.replace(arg)
                processed += 1
            })

        if ((references.length - processed) === 1) {
            removeHelperImport(j, rootScope, helperLocal)
        }
    })
}

export default wrap(transformAST)
