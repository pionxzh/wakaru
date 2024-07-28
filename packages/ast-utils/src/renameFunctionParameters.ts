import { renameIdentifier } from './reference'
import type { Scope } from 'ast-types/lib/scope'
import type { ArrowFunctionExpression, FunctionDeclaration, FunctionExpression, JSCodeshift } from 'jscodeshift'

export function renameFunctionParameters(j: JSCodeshift, node: FunctionDeclaration | FunctionExpression | ArrowFunctionExpression, parameters: string[]): void {
    if (
        !j.FunctionDeclaration.check(node)
        && !j.FunctionExpression.check(node)
        && !j.ArrowFunctionExpression.check(node)
    ) return

    const targetScope = j(node).get().scope as Scope | undefined
    if (!targetScope) return

    node.params.forEach((param, index) => {
        if (param.type === 'Identifier') {
            const oldName = param.name
            const newName = parameters[index]
            if (!newName || oldName === newName) return

            /**
             * Skip if the old name is declared multiple times
             * it means the parameter is shadowed by another variable
             * in the same scope
             */
            const bindings = targetScope.getBindings()
            if (bindings[oldName]?.length > 1) {
                param.name = newName
                return
            }

            renameIdentifier(j, targetScope, oldName, newName)
        }
    })
}
