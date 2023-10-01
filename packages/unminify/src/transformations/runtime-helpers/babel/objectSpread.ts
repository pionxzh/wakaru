import { findReferences } from '@unminify-kit/ast-utils'
import { findHelperLocals, removeHelperImport } from '../../../utils/import'
import { isHelperFunctionCall } from '../../../utils/isHelperFunctionCall'
import wrap from '../../../wrapAstTransformation'
import type { SharedParams } from '../../../utils/types'
import type { ASTTransformation } from '../../../wrapAstTransformation'
import type { Scope } from 'ast-types/lib/scope'
import type { ObjectExpression } from 'jscodeshift'

/**
 * Restore object spread syntax from `@babel/runtime/helpers/objectSpread2` helper.
 *
 * @see https://github.com/babel/babel/blob/main/packages/babel-helpers/src/helpers/objectSpread2.js
 */
export const transformAST: ASTTransformation<SharedParams> = (context, params) => {
    /**
     * `objectSpread2` was introduced in Babel v7.5.0
     */
    const moduleName = '@babel/runtime/helpers/objectSpread2'
    const moduleEsmName = '@babel/runtime/helpers/esm/objectSpread2'
    const fallbackModuleName = '@babel/runtime/helpers/objectSpread'
    const fallbackModuleEsmName = '@babel/runtime/helpers/esm/objectSpread'

    const { root, j } = context
    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    const helperLocals = [
        ...findHelperLocals(context, params, moduleName, moduleEsmName),
        ...findHelperLocals(context, params, fallbackModuleName, fallbackModuleEsmName),
    ]
    helperLocals.forEach((helperLocal) => {
        const references = findReferences(j, rootScope, helperLocal).length

        const collection = root
            // objectSpread({}, foo)
            .find(j.CallExpression)
            .filter(path => isHelperFunctionCall(j, path.node, helperLocal))

        collection
            .paths()
            .reverse()
            .forEach((path) => {
                const properties: ObjectExpression['properties'] = []

                for (const arg of path.node.arguments) {
                    if (j.ObjectExpression.check(arg)) {
                        properties.push(...arg.properties)
                    }
                    else if (j.SpreadElement.check(arg)) {
                        properties.push(arg)
                    }
                    else {
                        properties.push(j.spreadElement(arg))
                    }
                }

                const spreadObject = j.objectExpression(properties)
                path.replace(spreadObject)
            })

        const found = collection.size()
        if ((references - found) === 1) {
            removeHelperImport(j, rootScope, helperLocal)
        }
    })
}

export default wrap(transformAST)
