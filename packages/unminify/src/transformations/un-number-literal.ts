import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * 1e3 -> 1000
 * -2e4 -> -20000
 *
 * @see https://babeljs.io/docs/en/babel-plugin-minify-numeric-literals
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.Literal)
        .forEach((path) => {
            const { node } = path
            const { value } = node
            if (typeof value !== 'number') return

            const raw = j(node).toSource().toLowerCase()
            if (raw.includes('e')) {
                path.replace(j.literal(value))
            }
        })
}

export default wrap(transformAST)
