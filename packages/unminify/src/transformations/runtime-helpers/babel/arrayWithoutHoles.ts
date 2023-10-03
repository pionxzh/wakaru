import { findReferences } from '@unminify-kit/ast-utils'
import { findHelperLocals, removeHelperImport } from '../../../utils/import'
import { isHelperFunctionCall } from '../../../utils/isHelperFunctionCall'
import wrap from '../../../wrapAstTransformation'
import type { SharedParams } from '../../../utils/types'
import type { ASTTransformation } from '../../../wrapAstTransformation'
import type { Scope } from 'ast-types/lib/scope'
import type { ArrayExpression } from 'jscodeshift'

/**
 * `@babel/runtime/helpers/arrayWithoutHoles` helper.
 *
 * Replace `empty slot` with `undefined` in ArrayExpression.
 *
 * ```ts
 * function arrayWithoutHoles(arr)
 * ```
 *
 * We can further optimize this by detecting if we are wrapped by `toConsumableArray`
 * and skip the replacement as spread operator will handle `empty` correctly.
 *
 * @see https://github.com/babel/babel/blob/b5d6c3c820af3c049b476df6e885fef33fa953f1/packages/babel-helpers/src/helpers.ts#L743-L749
 */
export const transformAST: ASTTransformation<SharedParams> = (context, params) => {
    const moduleName = '@babel/runtime/helpers/arrayWithoutHoles'
    const moduleEsmName = '@babel/runtime/helpers/esm/arrayWithoutHoles'

    const { root, j } = context
    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    const helperLocals = findHelperLocals(context, params, moduleName, moduleEsmName)
    helperLocals.forEach((helperLocal) => {
        const references = findReferences(j, rootScope, helperLocal).length

        const found = root
            // arrayWithoutHoles([...])
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
