import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Remove `Object.defineProperty(exports, '__esModule', { value: true })`
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    const defineEsModule = root.find(j.ExpressionStatement, {
        expression: {
            type: 'CallExpression',
            callee: {
                type: 'MemberExpression',
                object: { type: 'Identifier', name: 'Object' },
                property: { type: 'Identifier', name: 'defineProperty' },
            },
            arguments: [
                { type: 'Identifier', name: 'exports' },
                { type: 'Literal', value: '__esModule' },
            ],
        },
    })
    defineEsModule.remove()
}

export default wrap(transformAST)
