import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { JSCodeshift } from 'jscodeshift'

/**
 * Converts minified `typeof` to its long form.
 *
 * @example
 * "typeof x < 'u'" => "typeof x !== 'undefined'"
 * "typeof x > 'u'" => "typeof x === 'undefined'"
 *
 * @see https://github.com/evanw/esbuild/blob/4e11b50fe3178ed0a78c077df78788d66304d379/internal/js_ast/js_ast_helpers.go#L151-L172
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.BinaryExpression, {
            left: { type: 'UnaryExpression', operator: 'typeof' },
            right: { type: 'StringLiteral', value: 'u' },
        })
        .forEach((p) => {
            const { left, operator } = p.node
            if (operator === '<') p.replace(toTypeofUndefined(j, left, '!=='))
            if (operator === '>') p.replace(toTypeofUndefined(j, left, '==='))
        })

    root
        .find(j.BinaryExpression, {
            left: { type: 'StringLiteral', value: 'u' },
            right: { type: 'UnaryExpression', operator: 'typeof' },
        })
        .forEach((p) => {
            const { right, operator } = p.node
            if (operator === '<') p.replace(toTypeofUndefined(j, right, '==='))
            if (operator === '>') p.replace(toTypeofUndefined(j, right, '!=='))
        })
}

function toTypeofUndefined(j: JSCodeshift, node: ExpressionKind, operator: '===' | '!==') {
    return j.binaryExpression(
        operator,
        node,
        j.stringLiteral('undefined'),
    )
}

export default wrap(transformAST)
