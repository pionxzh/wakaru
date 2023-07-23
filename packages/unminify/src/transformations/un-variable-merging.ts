import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * var a = 1, b = true, c = func(d)
 * ->
 * var a = 1
 * var b = true
 * var c = func(d)
 * TODO: handle for loop
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
        .forEach((p) => {
            if (j.ForStatement.check(p.parent?.node)) return

            const { kind, declarations } = p.node
            j(p).replaceWith(declarations.map(d => j.variableDeclaration(kind, [d])))
        })
}

export default wrap(transformAST)
