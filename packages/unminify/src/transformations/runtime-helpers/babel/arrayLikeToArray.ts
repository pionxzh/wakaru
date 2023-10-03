import { findReferences, isNumber } from '@unminify-kit/ast-utils'
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
 * ```ts
 * function arrayLikeToArray(arr, len?: number)
 * ```
 *
 * Note: Semantically, this is not the same as what `arrayWithoutHoles`
 * does, but currently we don't see other usage of `arrayLikeToArray`.
 *
 * We can further optimize this by detecting if we are wrapped by `toConsumableArray`
 * and skip the replacement as spread operator will handle `empty` correctly.
 *
 * @see https://github.com/babel/babel/blob/b5d6c3c820af3c049b476df6e885fef33fa953f1/packages/babel-helpers/src/helpers.ts#L789-L795
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
                if (!isHelperFunctionCall(j, path.node, helperLocal)) return false

                const argLength = path.node.arguments.length
                if (argLength === 0 || argLength > 2) return false

                if (!j.ArrayExpression.check(path.node.arguments[0])) return false

                if (argLength === 2) {
                    const secondArg = path.node.arguments[1]
                    return j.Literal.check(secondArg) && isNumber(secondArg.value)
                }

                return true
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
