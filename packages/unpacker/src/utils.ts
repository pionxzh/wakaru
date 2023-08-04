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
            j(node)
                .find(j.Identifier, { name: param.name })
                .filter(path => path.scope.node === node)
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

export function prettierFormat(code: string) {
    return prettier.format(code, {
        parser: 'babel',
        plugins: [babelParser],
    })
}
