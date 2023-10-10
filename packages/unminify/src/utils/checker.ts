import type { ASTNode, BigIntLiteral, BinaryExpression, BooleanLiteral, JSCodeshift, NullLiteral, NumericLiteral, RegExpLiteral, StringLiteral, TemplateLiteral } from 'jscodeshift'

export function areNodesEqual(j: JSCodeshift, node1: ASTNode, node2: ASTNode): boolean {
    return j(node1).toSource() === j(node2).toSource()
}

/**
 * Check if node is `true` literal
 */
export function isTrue(j: JSCodeshift, node: ASTNode): node is BooleanLiteral {
    return j.BooleanLiteral.check(node) && node.value === true
}

/**
 * Check if node is `false` literal
 */
export function isFalse(j: JSCodeshift, node: ASTNode): node is BooleanLiteral {
    return j.BooleanLiteral.check(node) && node.value === false
}

/**
 * Check if node is `null` literal
 */
export function isNull(j: JSCodeshift, node: ASTNode): node is NullLiteral {
    return j.NullLiteral.check(node) && node.value === null
}

/**
 * Check if node is `undefined` identifier or `void 0`
 */
export function isUndefined(j: JSCodeshift, node: ASTNode) {
    return isVoid0(j, node)
    || (j.Identifier.check(node) && node.name === 'undefined')
}

/**
 * Check if node is `void 0`
 */
export function isVoid0(j: JSCodeshift, node: ASTNode) {
    return j.UnaryExpression.check(node) && node.operator === 'void' && j.NumericLiteral.check(node.argument) && node.argument.value === 0
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

export function isValueLiteral(j: JSCodeshift, node: ASTNode): node is StringLiteral | NumericLiteral | BooleanLiteral | NullLiteral | BigIntLiteral | RegExpLiteral | TemplateLiteral {
    return j.StringLiteral.check(node)
    || j.NumericLiteral.check(node)
    || j.BooleanLiteral.check(node)
    || j.NullLiteral.check(node)
    || j.BigIntLiteral.check(node)
    || j.RegExpLiteral.check(node)
    || j.TemplateLiteral.check(node)
}
