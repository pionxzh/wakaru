import { splitVariableDeclarators } from '@unminify-kit/ast-utils'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * ```js
 * var a = 1, b = true, c = func(d)
 * ->
 * var a = 1
 * var b = true
 * var c = func(d)
 * ```
 *
 * @see https://babeljs.io/docs/en/babel-plugin-transform-merge-sibling-variables
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    splitVariableDeclarators(j, root)
}

export default wrap(transformAST)
