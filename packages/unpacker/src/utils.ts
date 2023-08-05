import type { ASTPath, ArrowFunctionExpression, Collection, ExpressionStatement, FunctionExpression, JSCodeshift, Node, Statement } from 'jscodeshift'
import prettier from 'prettier/standalone'
import babelParser from 'prettier/parser-babel'

export function isTopLevel(j: JSCodeshift, node: ASTPath<Node>): boolean {
    return j.Program.check(node.parentPath.node)
}

export function pruneComments(j: JSCodeshift, collection: Collection<any>): void {
    // @ts-expect-error - Comment type is wrong
    collection.find(j.Comment).forEach(path => path.prune())
}

export function renameFunctionParameters(j: JSCodeshift, node: FunctionExpression | ArrowFunctionExpression, parameters: string[]): void {
    node.params.forEach((param, index) => {
        if (param.type === 'Identifier') {
            const oldName = param.name
            const newName = parameters[index]

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

export function isIIFE(node: Statement): node is ExpressionStatement {
    if (node.type !== 'ExpressionStatement') return false
    const expression = (node as ExpressionStatement).expression
    if (expression.type !== 'CallExpression') return false
    const callee = expression.callee
    return callee.type === 'FunctionExpression'
        || callee.type === 'ArrowFunctionExpression'
}

export function prettierFormat(code: string) {
    return prettier.format(code, {
        parser: 'babel',
        plugins: [babelParser],
    })
}
