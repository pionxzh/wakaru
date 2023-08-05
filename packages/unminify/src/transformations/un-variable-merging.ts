import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * var a = 1, b = true, c = func(d)
 * ->
 * var a = 1
 * var b = true
 * var c = func(d)
 *
 * @see https://babeljs.io/docs/en/babel-plugin-transform-merge-sibling-variables
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.VariableDeclaration, {
            declarations: [
                {
                    type: 'VariableDeclarator',
                    id: { type: 'Identifier' },
                },
            ],
        })
        .filter((path) => {
            if (path.parent?.node.type === 'ForStatement') return false
            return path.node.declarations.length > 1
        })
        .forEach((p) => {
            const { kind, declarations } = p.node
            j(p).replaceWith(declarations.map(d => j.variableDeclaration(kind, [d])))
        })
}

export default wrap(transformAST)
