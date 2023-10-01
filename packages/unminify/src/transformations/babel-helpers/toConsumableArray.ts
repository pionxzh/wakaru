import { findReferences } from '@unminify-kit/ast-utils'
import { findHelperLocals, removeHelperImport } from '../../utils/import'
import { isHelperFunctionCall } from '../../utils/isHelperFunctionCall'
import wrap from '../../wrapAstTransformation'
import type { SharedParams } from '../../utils/types'
import type { ASTTransformation } from '../../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { Scope } from 'ast-types/lib/scope'

/**
 * Restores spread operator from `@babel/runtime/helpers/toConsumableArray` helper.
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-spread
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

export default wrap(transformAST)
