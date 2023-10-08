import { findReferences } from '@wakaru/ast-utils'
import { findHelperLocals, removeHelperImport } from '../../../utils/import'
import { isHelperFunctionCall } from '../../../utils/isHelperFunctionCall'
import wrap from '../../../wrapAstTransformation'
import { handleSpreadHelper } from './_spread'
import type { SharedParams } from '../../../utils/types'
import type { ASTTransformation, Context } from '../../../wrapAstTransformation'
import type { Scope } from 'ast-types/lib/scope'
import type { CallExpression, Identifier } from 'jscodeshift'

/**
 * Restore object spread syntax from `@babel/runtime/helpers/objectSpread2` helper.
 *
 *
 * ```ts
 * function extends(target, ...sources)
 * ```
 *
 * @example
 * babelHelpers.extends({}, (babelHelpers.objectDestructuringEmpty(this.props), this.props));
 * ->
 * { ...this.props }
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-object-rest-spread
 * @see https://github.com/babel/babel/blob/main/packages/babel-helpers/src/helpers/objectSpread2.js
 */
export const transformAST: ASTTransformation<SharedParams> = (context, params) => {
    handleObjectDestructuringEmpty(context, params)

    /**
     * `objectSpread2` was introduced in Babel v7.5.0
     */
    const moduleName = '@babel/runtime/helpers/objectSpread2'
    const moduleEsmName = '@babel/runtime/helpers/esm/objectSpread2'
    const fallbackModuleName = '@babel/runtime/helpers/objectSpread'
    const fallbackModuleEsmName = '@babel/runtime/helpers/esm/objectSpread'

    const helperLocals = [
        ...findHelperLocals(context, params, moduleName, moduleEsmName),
        ...findHelperLocals(context, params, fallbackModuleName, fallbackModuleEsmName),
    ]
    handleSpreadHelper(context, helperLocals)
}

function handleObjectDestructuringEmpty(context: Context, params: SharedParams) {
    const { root, j } = context
    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    /**
     * `objectDestructuringEmpty` is a checker that throws error if the input is `null`.
     * Let's just remove it.
     */
    const checkerName = '@babel/runtime/helpers/objectDestructuringEmpty'
    const checkerEsmName = '@babel/runtime/helpers/esm/objectDestructuringEmpty'
    const checkerLocals = findHelperLocals(context, params, checkerName, checkerEsmName)
    checkerLocals.forEach((helperLocal) => {
        const references = findReferences(j, rootScope, helperLocal).length

        const found = root
            // (objectDestructuringEmpty(a), a)
            .find(j.SequenceExpression, {
                expressions: [
                    (expression) => {
                        return isHelperFunctionCall(j, expression, helperLocal)
                        && expression.arguments.length === 1
                        && j.Identifier.check(expression.arguments[0])
                    },
                    { type: 'Identifier' },
                ],
            })
            .filter((path) => {
                // the argument of `objectDestructuringEmpty` is the same as the second expression
                const [first, second] = path.value.expressions as [CallExpression, Identifier]
                return (first.arguments[0] as Identifier).name === second.name
            })
            .forEach((path) => {
                path.replace(path.value.expressions[1])
            })
            .size()

        if ((references - found) === 1) {
            removeHelperImport(j, rootScope, helperLocal)
        }
    })
}

export default wrap(transformAST)
