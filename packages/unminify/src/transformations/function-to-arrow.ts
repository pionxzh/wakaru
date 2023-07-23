import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import { containsThisExpression } from '../utils/containsThisExpression'
import { createArrowFunctionExpression } from '../utils/createArrowFunctionExpression'

/**
 * function add(a, b) { return a + b }
 * ->
 * const add = (a, b) => a + b
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.FunctionDeclaration)
        .filter((path) => {
            if (j.MethodDefinition.check(path.parent)) return false
            if (j.Property.check(path.parent)) return false
            if (containsThisExpression(path.node)) return false
            return true
        })
        .forEach((path) => {
            const { node } = path
            const { id } = node
            if (!id) return
            const arrowFunctionExpression = createArrowFunctionExpression(j, node)
            const variableDeclaration = j.variableDeclaration('const', [
                j.variableDeclarator(id, arrowFunctionExpression),
            ])
            j(path).replaceWith(variableDeclaration)
        })
}

export default wrap(transformAST)
