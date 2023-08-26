import isValidIdentifier from '../utils/isValidIdentifier'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { Literal } from 'jscodeshift'

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
            property: { type: 'Literal' },
        })
        .forEach((p) => {
            const property = p.node.property as Literal
            if (typeof property.value !== 'string') return

            if (property.value.match(/^\d+$/)) {
                const newProp = Number.parseInt(property.value, 10)
                if (newProp.toString() === property.value) {
                    property.value = newProp
                }
            }
            else if (isValidIdentifier(property.value)) {
                p.node.property = j.identifier(property.value)
                p.node.computed = false
            }
        })
}

export default wrap(transformAST)
