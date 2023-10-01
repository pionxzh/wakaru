import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { CallExpression, JSCodeshift } from 'jscodeshift'

/**
 * Checks if the expression is a call to the helper function.
 *
 * @example
 * // matches
 * helperName(...)
 * helperName.default(...)
 * (0, helperName)(...)
 * (0, helperName.default)(...)
 */
export function isHelperFunctionCall(
    j: JSCodeshift,
    expression: ExpressionKind | null | undefined,
    helperName: string,
): expression is CallExpression {
    if (!j.CallExpression.check(expression)) return false

    let callee = expression.callee
    if (j.SequenceExpression.check(callee)) {
        if (
            callee.expressions.length === 2
            && j.Literal.check(callee.expressions[0])
            && callee.expressions[0].value === 0
        ) {
            callee = callee.expressions[1]
        }
    }

    if (j.Identifier.check(callee)) {
        return callee.name === helperName
    }
    if (j.MemberExpression.check(callee)) {
        return (
            j.Identifier.check(callee.object)
            && callee.object.name === helperName
            && j.Identifier.check(callee.property)
            && callee.property.name === 'default'
        )
    }

    return false
}
