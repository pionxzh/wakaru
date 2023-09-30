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

            renameIdentifier(j, targetScope, oldName, newName)
        }
    })
}
