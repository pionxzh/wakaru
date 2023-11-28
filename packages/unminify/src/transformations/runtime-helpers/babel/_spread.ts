import { findReferences } from '@wakaru/ast-utils'
import { removeHelperImport } from '../../../utils/import'
import { isHelperFunctionCall } from '../../../utils/isHelperFunctionCall'
import type { Context } from '@wakaru/ast-utils'
import type { Scope } from 'ast-types/lib/scope'
import type { ObjectExpression } from 'jscodeshift'

export function handleSpreadHelper(context: Context, helperLocals: string[]) {
    const { root, j } = context
    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    helperLocals.forEach((helperLocal) => {
        const references = findReferences(j, rootScope, helperLocal).length

        const collection = root
            // objectSpread({}, foo, ...)
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
