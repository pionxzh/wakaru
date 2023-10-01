import { findReferences } from '@unminify-kit/ast-utils'
import { findHelperLocals, removeHelperImport } from '../../../utils/import'
import { isHelperFunctionCall } from '../../../utils/isHelperFunctionCall'
import wrap from '../../../wrapAstTransformation'
import type { SharedParams } from '../../../utils/types'
import type { ASTTransformation } from '../../../wrapAstTransformation'
import type { Scope } from 'ast-types/lib/scope'
import type { ArrayExpression } from 'jscodeshift'

/**
 * `@babel/runtime/helpers/arrayLikeToArray` helper.
 *
 * Replace `empty slot` with `undefined` in ArrayExpression.
 *
 * Note: Semantically, this is not the same as what `arrayWithoutHoles`
 * does, but currently we don't see other usage of `arrayLikeToArray`.
 *
 * We can further optimize this by detecting if we are wrapped by `toConsumableArray`
 * and skip the replacement as spread operator will handle `empty` correctly.
 */
export const transformAST: ASTTransformation<SharedParams> = (context, params) => {
    const moduleName = '@babel/runtime/helpers/arrayLikeToArray'
    const moduleEsmName = '@babel/runtime/helpers/esm/arrayLikeToArray'

    const { root, j } = context
    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    const helperLocals = findHelperLocals(context, params, moduleName, moduleEsmName)
    helperLocals.forEach((helperLocal) => {
        const references = findReferences(j, rootScope, helperLocal).length

        const found = root
            // arrayLikeToArray([...])
            .find(j.CallExpression)
            .filter((path) => {
                return isHelperFunctionCall(j, path.node, helperLocal)
                && path.node.arguments.length === 1
                && j.ArrayExpression.check(path.node.arguments[0])
            })
            .forEach((path) => {
                const arr = path.node.arguments[0] as ArrayExpression
                const elements = arr.elements.map(element => element ?? j.identifier('undefined'))
                path.replace(j.arrayExpression(elements))
            })
            .size()

        if ((references - found) === 1) {
            removeHelperImport(j, rootScope, helperLocal)
        }
    })
}

export default wrap(transformAST)
