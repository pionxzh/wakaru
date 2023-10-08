import { findReferences } from '@unminify-kit/ast-utils'
import { mergeComments } from '../../../utils/comments'
import { findHelperLocals, removeHelperImport } from '../../../utils/import'
import wrap from '../../../wrapAstTransformation'
import type { SharedParams } from '../../../utils/types'
import type { ASTTransformation } from '../../../wrapAstTransformation'
import type { Scope } from 'ast-types/lib/scope'
import type { CallExpression } from 'jscodeshift'

export const NAMESPACE_IMPORT_HINT = '* @hint namespace-import '

/**
 * Restores wildcard import from `@babel/runtime/helpers/interopRequireWildcard` helper.
 * A hint is added to the require call to indicate that it is a namespace import.
 * So that we can transform it into an namespace import later.
 *
 * ```ts
 * function interopRequireWildcard(obj, nodeInterop?: boolean)
 * ```
 *
 * @example
 * var _a = interopRequireWildcard(require("a"));
 * ->
 * var _a = /** @hint namespace-import *\/require("a");
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-modules-commonjs
 */
export const transformAST: ASTTransformation<SharedParams> = (context, params) => {
    const moduleName = '@babel/runtime/helpers/interopRequireWildcard'
    const moduleEsmName = '@babel/runtime/helpers/esm/interopRequireWildcard'

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
                    && parentNode.arguments.length >= 1
                    && parentNode.arguments.length <= 2
            })
            .forEach((path) => {
                const callExpression = path.parent?.node as CallExpression
                const arg = callExpression.arguments[0]
                if (j.SpreadElement.check(arg)) return

                // var source = require("a")/** @hint namespace-import */;
                const hintComment = j.commentBlock(NAMESPACE_IMPORT_HINT, false, true)
                mergeComments(arg, [hintComment])

                path.parent.replace(arg)
                processed += 1
            })

        if ((references.length - processed) === 1) {
            removeHelperImport(j, rootScope, helperLocal)
        }
    })
}

export default wrap(transformAST)
