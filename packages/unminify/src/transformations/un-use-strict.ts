import { mergeComments } from '@wakaru/ast-utils/comments'
import { wrapAstTransformation } from '@wakaru/ast-utils/wrapAstTransformation'
import type { ASTTransformation } from '@wakaru/ast-utils/wrapAstTransformation'

/**
 * Remove the 'use strict' directives
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-minify-booleans
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    const useStrict = root
        .find(j.Directive, {
            value: { type: 'DirectiveLiteral', value: 'use strict' },
        })
        .forEach((path) => {
            const { node } = path
            const { comments } = node
            if (comments) {
                const parentNode = path.parent.node
                if (j.Program.check(parentNode) || j.BlockStatement.check(parentNode)) {
                    // @ts-expect-error Directive is not included in the StatementKind
                    const index = parentNode.body.indexOf(node)
                    const nextNode = parentNode.body[index + 1] || parentNode.body[0]
                    mergeComments(nextNode, comments)
                }
            }
        })

    useStrict.remove()
}

export default wrapAstTransformation(transformAST)
