import { isBoolean } from '@unminify-kit/ast-utils'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { BinaryExpression, JSCodeshift } from 'jscodeshift'

/**
 * Apply de Morgan's laws to negate a condition.
 */
export function negateCondition(j: JSCodeshift, condition: ExpressionKind): ExpressionKind {
    if (j.UnaryExpression.check(condition) && condition.operator === '!') {
        return condition.argument
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

    if (j.Literal.check(condition) && isBoolean(condition.value)) {
        return j.literal(!condition.value)
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
