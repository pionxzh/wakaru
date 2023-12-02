import { wrapAstTransformation } from '@wakaru/ast-utils/wrapAstTransformation'
import { isVoid0 } from '../utils/checker'
import type { ASTTransformation } from '@wakaru/ast-utils/wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { BinaryExpression, JSCodeshift } from 'jscodeshift'

const equalityOperators = [
    '==',
    '===',
    '!=',
    '!==',
]
const relationalOperators = [
    '<',
    '>',
    '<=',
    '>=',
]

const operatorFlipMap = new Map<BinaryExpression['operator'], BinaryExpression['operator']>([
    ['==', '!='],
    ['===', '!=='],
    ['!=', '=='],
    ['!==', '==='],
    ['<', '>'],
    ['>', '<'],
    ['<=', '>='],
    ['>=', '<='],
])

const commonValueIdentifiers = [
    'undefined',
    'NaN',
    'Infinity',
]

const isLeftValid = (j: JSCodeshift, node: ExpressionKind) => {
    if (isVoid0(j, node)) return true

    if (
        j.NullLiteral.check(node)
     || j.BooleanLiteral.check(node)
     || j.NumericLiteral.check(node)
     || j.StringLiteral.check(node)
    ) {
        return true
    }

    if (j.Identifier.check(node)) {
        return commonValueIdentifiers.includes(node.name)
    }

    if (j.UnaryExpression.check(node)) {
        return j.Identifier.check(node.argument) && commonValueIdentifiers.includes(node.argument.name)
    }

    if (j.TemplateLiteral.check(node)) {
        return node.expressions.length === 0
    }

    return false
}

const isRightValid = (j: JSCodeshift, node: ExpressionKind) => {
    const right = j.UnaryExpression.check(node) ? node.argument : node
    return j.Identifier.check(right)
    || j.MemberExpression.check(right)
    || j.CallExpression.check(right)
}
/**
 * Flips comparisons that are in the form of "literal comes first"
 * to "literal comes second".
 *
 * @example
 * `1 < a` -> `a > 1`
 * `undefined === foo` -> `foo === undefined`
 * `null !== bar` -> `bar !== null`
 *
 * @see https://babeljs.io/docs/babel-plugin-minify-flip-comparisons (reversed)
 * @see https://eslint.org/docs/latest/rules/yoda
 * @see https://github.com/eslint/eslint/blob/main/lib/rules/yoda.js
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.BinaryExpression, {
            operator: op => equalityOperators.includes(op) || relationalOperators.includes(op),
        })
        .forEach((p) => {
            const { node } = p
            const { operator, left, right } = node

            if (isRightValid(j, right) && isLeftValid(j, left)) {
                node.left = right
                node.right = left
                if (relationalOperators.includes(operator)) {
                    node.operator = operatorFlipMap.get(operator) || operator
                }
            }
        })
}

export default wrapAstTransformation(transformAST)
