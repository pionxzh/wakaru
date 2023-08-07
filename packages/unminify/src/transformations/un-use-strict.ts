import { isTopLevel } from '@unminify-kit/ast-utils'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Remove the 'use strict' directives
 *
 * @see https://babeljs.io/docs/en/babel-plugin-transform-minify-booleans
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    const leadingComment = root
        .find(j.Program)
        .get('body', 0)
        .node.leadingComments

    const useStrict = root
        .find(j.ExpressionStatement, {
            expression: { type: 'Literal', value: 'use strict' },
        })

    useStrict.remove()

    if (leadingComment && useStrict.some(path => isTopLevel(j, path))) {
        root.get().node.comments = leadingComment
    }
}

export default wrap(transformAST)
