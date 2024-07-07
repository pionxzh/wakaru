import { j } from '@wakaru/shared/jscodeshift'

export const patternKindTypes = [
    j.Identifier,
    j.RestElement,
    j.SpreadElementPattern,
    j.PropertyPattern,
    j.ObjectPattern,
    j.ArrayPattern,
    j.AssignmentPattern,
    j.SpreadPropertyPattern,
    j.PrivateName,
    j.JSXIdentifier,
]
export const memberExpressionKindTypes = [
    j.MemberExpression,
    j.OptionalMemberExpression,
    j.JSXMemberExpression,
]
