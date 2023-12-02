import { findReferences } from '@wakaru/ast-utils/reference'
import { type ASTTransformation, wrapAstTransformation } from '@wakaru/ast-utils/wrapAstTransformation'
import { findHelperLocals, removeHelperImport } from '../../../utils/import'
import { isHelperFunctionCall } from '../../../utils/isHelperFunctionCall'
import type { SharedParams } from '../../../utils/types'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { Scope } from 'ast-types/lib/scope'
import type { CallExpression, Identifier, NumericLiteral, VariableDeclarator } from 'jscodeshift'

/**
 * Restores array destructuring from `@babel/runtime/helpers/slicedToArray` helper.
 *
 * ```ts
 * function slicedToArray(arr, len?: number)
 * ```
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
 * @see https://github.com/babel/babel/blob/b5d6c3c820af3c049b476df6e885fef33fa953f1/packages/babel-helpers/src/helpers.ts#L679-L693
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
            .find(j.VariableDeclarator, {
                id: { type: 'Identifier' },
                init: (init) => {
                    return isHelperFunctionCall(j, init, helperLocal)
                    && init.arguments.length === 2
                    && j.NumericLiteral.check(init.arguments[1])
                },
            })
            .forEach((path) => {
                const decl = path.node as VariableDeclarator
                const tempVariable = decl.id as Identifier
                const wrappedExpression = (decl.init as CallExpression).arguments[0] as ExpressionKind
                const length = ((decl.init as CallExpression).arguments[1] as NumericLiteral).value as number

                if (length === 0) {
                    // var [] = wrappedExpression
                    path.replace(j.variableDeclarator(j.arrayPattern([]), wrappedExpression))
                }
                else {
                    path.replace(j.variableDeclarator(tempVariable, wrappedExpression))
                }
            })
            .size()

        if ((references - found) === 1) {
            removeHelperImport(j, rootScope, helperLocal)
        }
    })
}

export default wrapAstTransformation(transformAST)
