import { findReferences, wrapAstTransformation } from '@wakaru/ast-utils'
import { findHelperLocals, removeHelperImport } from '../../../utils/import'
import { isHelperFunctionCall } from '../../../utils/isHelperFunctionCall'
import type { SharedParams } from '../../../utils/types'
import type { ASTTransformation } from '@wakaru/ast-utils'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { Scope } from 'ast-types/lib/scope'

/**
 * Restores spread operator from `@babel/runtime/helpers/toConsumableArray` helper.
 *
 * ```ts
 * function toConsumableArray(arr)
 * ```
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-spread
 * @see https://github.com/babel/babel/blob/b5d6c3c820af3c049b476df6e885fef33fa953f1/packages/babel-helpers/src/helpers.ts#L727-L741
 */
export const transformAST: ASTTransformation<SharedParams> = (context, params) => {
    const moduleName = '@babel/runtime/helpers/toConsumableArray'
    const moduleEsmName = '@babel/runtime/helpers/esm/toConsumableArray'

    const { root, j } = context
    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    const helperLocals = findHelperLocals(context, params, moduleName, moduleEsmName)
    helperLocals.forEach((helperLocal) => {
        const references = findReferences(j, rootScope, helperLocal).length

        const found = root
            // toConsumableArray(a)
            .find(j.CallExpression)
            .filter((path) => {
                return isHelperFunctionCall(j, path.node, helperLocal)
                && path.node.arguments.length === 1
                && j.Expression.check(path.node.arguments[0])
            })
            .forEach((path) => {
                path.replace(j.arrayExpression([j.spreadElement(path.node.arguments[0] as ExpressionKind)]))
            })
            .size()

        if ((references - found) === 1) {
            removeHelperImport(j, rootScope, helperLocal)
        }
    })
}

export default wrapAstTransformation(transformAST)
