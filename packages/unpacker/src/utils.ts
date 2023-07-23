import type { ASTPath, ArrowFunctionExpression, ExpressionStatement, FunctionExpression, JSCodeshift, Node, Statement } from 'jscodeshift'

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

export function isIIFE(node: Statement): node is ExpressionStatement {
    if (node.type !== 'ExpressionStatement') return false
    const expression = (node as ExpressionStatement).expression
    if (expression.type !== 'CallExpression') return false
    const callee = expression.callee
    return callee.type === 'FunctionExpression'
        || callee.type === 'ArrowFunctionExpression'
}
