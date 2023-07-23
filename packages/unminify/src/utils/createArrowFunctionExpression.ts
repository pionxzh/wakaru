import type { FunctionDeclaration, JSCodeshift } from 'jscodeshift'

export function createArrowFunctionExpression(j: JSCodeshift, fn: FunctionDeclaration) {
    const { params, body, async, comments } = fn
    const arrowFunction = j.arrowFunctionExpression(
        params,
        body,
        false,
    )
    arrowFunction.async = async
    arrowFunction.comments = comments
    return arrowFunction
}
