import type { ASTPath, ArrowFunctionExpression, FunctionExpression, JSCodeshift, Node } from 'jscodeshift'

export function isTopLevel(j: JSCodeshift, node: ASTPath<Node>): boolean {
    return j.Program.check(node.parentPath.node)
}

export function renameFunctionParameters(j: JSCodeshift, node: FunctionExpression | ArrowFunctionExpression, parameters: string[]): void {
    node.params.forEach((param, index) => {
        if (param.type === 'Identifier') {
            j(node)
                .find(j.Identifier, { name: param.name })
                .forEach((path) => {
                    path.node.name = parameters[index]
                })
        }
    })
}
