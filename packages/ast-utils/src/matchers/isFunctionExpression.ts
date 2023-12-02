import type { ArrowFunctionExpression, FunctionExpression, JSCodeshift } from 'jscodeshift'

export function isFunctionExpression(j: JSCodeshift, node: any): node is FunctionExpression | ArrowFunctionExpression {
    return j.FunctionExpression.check(node)
    || j.ArrowFunctionExpression.check(node)
}
