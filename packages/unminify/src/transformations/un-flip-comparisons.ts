import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { Identifier } from '@babel/types'
import type { BinaryExpression, Literal } from 'jscodeshift'

const qualityOperators = ['==', '===', '!=', '!==']
const relationalOperators = ['<', '>', '<=', '>=']
const invertOperator = (operator: BinaryExpression['operator']): BinaryExpression['operator'] => {
    switch (operator) {
        case '==': return '!='
        case '===': return '!=='
        case '!=': return '=='
        case '!==': return '==='
        case '<': return '>'
        case '>': return '<'
        case '<=': return '>='
        case '>=': return '<='
        default: return operator
    }
}

const isUndefined = (node: any): node is Identifier => {
    return node.type === 'Identifier' && node.name === 'undefined'
}

const isNull = (node: any): node is Literal => {
    return node.type === 'Literal' && node.value === null
}

/**
 * `undefined === foo` -> `foo === undefined`
 * `null !== bar` -> `bar !== null`
 * `1 < a` -> `a > 1`
 * @see https://babeljs.io/docs/en/babel-plugin-minify-flip-comparisons (reversed)
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.BinaryExpression, {
            operator: op => qualityOperators.includes(op) || relationalOperators.includes(op),
        })
        .forEach((p) => {
            const { node } = p
            const { operator, left, right } = node

            if (!j.Identifier.check(right) && !(j.UnaryExpression.check(right) && j.Identifier.check(right.argument))) return

            if (isNull(left) || isUndefined(left)) {
                node.left = right
                node.right = left
                if (relationalOperators.includes(operator)) {
                    node.operator = invertOperator(operator)
                }
            }
            // @ts-expect-error - value is not a property of Literal
            else if (j.Literal.check(left) && (typeof left.value === 'number' || typeof left.value === 'string')) {
                node.left = right
                node.right = left
                if (relationalOperators.includes(operator)) {
                    node.operator = invertOperator(operator)
                }
            }
        })
}

export default wrap(transformAST)
