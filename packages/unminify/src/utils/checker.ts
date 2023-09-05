import type { ASTNode, BinaryExpression, JSCodeshift } from 'jscodeshift'

export function isNull(j: JSCodeshift, node: ASTNode) {
    return j.Literal.check(node) && node.value === null
}

export function isUndefined(j: JSCodeshift, node: ASTNode) {
    return isVoid0(j, node)
    || (j.Identifier.check(node) && node.name === 'undefined')
}

export function isVoid0(j: JSCodeshift, node: ASTNode) {
    return j.UnaryExpression.check(node) && node.operator === 'void' && j.Literal.check(node.argument) && node.argument.value === 0
}

export function isNotNullBinary(j: JSCodeshift, node: ASTNode): node is BinaryExpression {
    return j.BinaryExpression.check(node)
    && node.operator === '!=='
    && (isNull(j, node.left) || isNull(j, node.right))
}

export function isNullBinary(j: JSCodeshift, node: ASTNode): node is BinaryExpression {
    return j.BinaryExpression.check(node)
    && node.operator === '==='
    && (isNull(j, node.left) || isNull(j, node.right))
}

export function isUndefinedBinary(j: JSCodeshift, node: ASTNode): node is BinaryExpression {
    return j.BinaryExpression.check(node)
    && node.operator === '==='
    && (isUndefined(j, node.left) || isUndefined(j, node.right))
}
