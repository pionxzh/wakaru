import { isLogicalNot } from './checker'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { BinaryExpression, ConditionalExpression, JSCodeshift, LogicalExpression } from 'jscodeshift'

/**
 * Convert a logical expression to a conditional expression.
 *
 * @example
 * a && b -> a ? b : false
 * a || b -> a ? true : b
 * a ?? b -> a !== null && a !== undefined ? a : b
 */
export function logicalExpressionToConditionalExpression(j: JSCodeshift, node: LogicalExpression): ConditionalExpression {
    const { left, right, operator } = node

    if (operator === '&&') {
        return j.conditionalExpression(left, right, j.booleanLiteral(false))
    }
    if (operator === '||') {
        return j.conditionalExpression(left, j.booleanLiteral(true), right)
    }
    if (operator === '??') {
        return j.conditionalExpression(
            j.logicalExpression('||',
                j.binaryExpression('!==', left, j.nullLiteral()),
                j.binaryExpression('!==', left, j.identifier('undefined')),
            ),
            left,
            right,
        )
    }

    throw new Error(`Unexpected operator ${operator} while converting logical expression to conditional expression with ${j(node).toSource()}`)
}

/**
 * Apply de Morgan's laws to negate a condition.
 */
export function negateCondition(j: JSCodeshift, condition: ExpressionKind): ExpressionKind {
    if (isLogicalNot(j, condition)) {
        return condition.argument
    }
    if (j.ConditionalExpression.check(condition)) {
        if (isLogicalNot(j, condition.consequent) || isLogicalNot(j, condition.alternate)) {
            return j.conditionalExpression(
                negateCondition(j, condition.test),
                negateCondition(j, condition.alternate),
                negateCondition(j, condition.consequent),
            )
        }
        return j.conditionalExpression(
            negateCondition(j, condition.test),
            condition.alternate,
            condition.consequent,
        )
    }
    if (j.LogicalExpression.check(condition) && (condition.operator === '&&' || condition.operator === '||')) {
        return j.logicalExpression(
            condition.operator === '&&' ? '||' : '&&',
            negateCondition(j, condition.left),
            negateCondition(j, condition.right),
        )
    }
    if (j.BinaryExpression.check(condition)) {
        return j.binaryExpression(
            getNegatedOperator(condition.operator),
            condition.left,
            condition.right,
        )
    }

    if (j.BooleanLiteral.check(condition)) {
        return j.booleanLiteral(!condition.value)
    }

    return j.unaryExpression('!', condition)
}

function getNegatedOperator(operator: BinaryExpression['operator']): BinaryExpression['operator'] {
    switch (operator) {
        case '==': return '!='
        case '===': return '!=='
        case '!=': return '=='
        case '!==': return '==='
        case '<': return '>='
        case '<=': return '>'
        case '>': return '<='
        case '>=': return '<'
        default: return operator
    }
}
