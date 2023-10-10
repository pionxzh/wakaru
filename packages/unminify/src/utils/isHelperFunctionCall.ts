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
 *
 * // if helperName contains a dot
 * helperName.foo(...)
 * helperName.default.foo(...)
 * (0, helperName.foo)(...)
 * (0, helperName.default.foo)(...)
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
            && j.NumericLiteral.check(callee.expressions[0])
            && callee.expressions[0].value === 0
        ) {
            callee = callee.expressions[1]
        }
    }

    if (helperName.includes('.')) {
        const [helper, helperProp] = helperName.split('.')
        if (j.MemberExpression.check(callee)) {
            return (
                j.Identifier.check(callee.object)
                && callee.object.name === helper
                && j.Identifier.check(callee.property)
                && callee.property.name === helperProp
            ) || (
                j.MemberExpression.check(callee.object)
                && j.Identifier.check(callee.object.object)
                && callee.object.object.name === helper
                && j.Identifier.check(callee.object.property)
                && callee.object.property.name === 'default'
                && j.Identifier.check(callee.property)
                && callee.property.name === helperProp
            )
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
