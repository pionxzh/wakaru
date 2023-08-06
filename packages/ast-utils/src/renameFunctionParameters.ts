import type { ArrowFunctionExpression, FunctionExpression, JSCodeshift } from 'jscodeshift'

export function renameFunctionParameters(j: JSCodeshift, node: FunctionExpression | ArrowFunctionExpression, parameters: string[]): void {
    node.params.forEach((param, index) => {
        if (param.type === 'Identifier') {
            const oldName = param.name
            const newName = parameters[index]
            if (!newName || oldName === newName) return

            // Only get the immediate function scope
            const functionScope = j(node).closestScope().get()

            // Check if the name is in the current scope and rename it
            if (functionScope.scope.getBindings()[oldName]) {
                j(functionScope)
                    .find(j.Identifier, { name: oldName })
                    .forEach((path) => {
                        // Exclude MemberExpression properties
                        if (!(path.parent.node.type === 'MemberExpression' && path.parent.node.property === path.node)
                            && path.scope.node === functionScope.node) {
                            path.node.name = newName
                        }
                    })
            }
        }
    })
}
