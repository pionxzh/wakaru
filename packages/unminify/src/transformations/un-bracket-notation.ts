import { isValidIdentifier, wrapAstTransformation } from '@wakaru/ast-utils'
import type { ASTTransformation } from '@wakaru/ast-utils'
import type { StringLiteral } from 'jscodeshift'

/**
 * Simplify bracket notation.
 *
 * @example
 * obj['bar'] -> obj.bar
 * obj['var'] -> obj['var']
 * arr['1'] -> arr[1]
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-member-expression-literals
 * @see https://github.com/terser/terser/blob/master/test/compress/properties.js
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.MemberExpression, {
            computed: true,
            property: { type: 'StringLiteral' },
        })
        .forEach((p) => {
            const property = p.node.property as StringLiteral
            if (property.value.match(/^\d+(\.\d+)?$/)) {
                const newProp = Number.parseFloat(property.value)
                if (newProp.toString() === property.value) {
                    p.node.property = j.numericLiteral(newProp)
                }
            }
            else if (isValidIdentifier(property.value)) {
                p.node.property = j.identifier(property.value)
                p.node.computed = false
            }
        })
}

export default wrapAstTransformation(transformAST)
