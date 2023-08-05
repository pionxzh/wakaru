import type { ExpressionStatement, Statement } from 'jscodeshift'

export function isIIFE(node: Statement): node is ExpressionStatement {
    if (node.type !== 'ExpressionStatement') return false
    const expression = (node as ExpressionStatement).expression
    if (expression.type !== 'CallExpression') return false
    const callee = expression.callee
    return callee.type === 'FunctionExpression'
        || callee.type === 'ArrowFunctionExpression'
}
