import { findReferences, isNumber } from '@unminify-kit/ast-utils'
import { findHelperLocals, removeHelperImport } from '../../../utils/import'
import { isHelperFunctionCall } from '../../../utils/isHelperFunctionCall'
import { removeDeclarationIfUnused } from '../../../utils/scope'
import wrap from '../../../wrapAstTransformation'
import type { SharedParams } from '../../../utils/types'
import type { ASTTransformation } from '../../../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { Scope } from 'ast-types/lib/scope'
import type { CallExpression, Identifier, Literal, VariableDeclarator } from 'jscodeshift'

/**
 * Restores array destructuring from `@babel/runtime/helpers/slicedToArray` helper.
 *
 * @example
 * var _ref = slicedToArray(a, 2)
 * var name = _ref[0]
 * var age = _ref[1]
 * ->
 * var _ref = a
 * var name = _ref[0]
 * var age = _ref[1]
 *
 * TODO: improve `for...of` loops output.
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-destructuring
 */
export const transformAST: ASTTransformation<SharedParams> = (context, params) => {
    const moduleName = '@babel/runtime/helpers/slicedToArray'
    const moduleEsmName = '@babel/runtime/helpers/esm/slicedToArray'

    const { root, j } = context
    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    const helperLocals = findHelperLocals(context, params, moduleName, moduleEsmName)
    helperLocals.forEach((helperLocal) => {
        const references = findReferences(j, rootScope, helperLocal).length

        const found = root
            // var _ref = slicedToArray(a, 2)
            .find(j.VariableDeclaration, {
                declarations: (declarations) => {
                    return declarations.length === 1
                    && j.VariableDeclarator.check(declarations[0])
                    && j.Identifier.check(declarations[0].id)
                    && isHelperFunctionCall(j, declarations[0].init, helperLocal)

                    && declarations[0].init.arguments.length === 2
                    && j.Literal.check(declarations[0].init.arguments[1])
                    && isNumber(declarations[0].init.arguments[1].value)
                },
            })
            .forEach((path) => {
                const decl = path.node.declarations[0] as VariableDeclarator
                const tempVariable = decl.id as Identifier
                const wrappedExpression = (decl.init as CallExpression).arguments[0] as ExpressionKind
                const length = ((decl.init as CallExpression).arguments[1] as Literal).value as number

                if (length === 0) {
                    // var [] = wrappedExpression
                    path.replace(j.variableDeclaration(path.node.kind, [
                        j.variableDeclarator(
                            j.arrayPattern([]),
                            wrappedExpression,
                        ),
                    ]))
                }
                else {
                    path.replace(j.variableDeclaration(path.node.kind, [
                        j.variableDeclarator(
                            tempVariable,
                            wrappedExpression,
                        ),
                    ]))
                }
                removeDeclarationIfUnused(j, path, helperLocal)
            })
            .size()

        if ((references - found) === 1) {
            removeHelperImport(j, rootScope, helperLocal)
        }
    })
}

export default wrap(transformAST)
