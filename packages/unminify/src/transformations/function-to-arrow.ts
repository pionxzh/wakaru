import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { FunctionDeclaration, JSCodeshift } from 'jscodeshift'

/**
 * function add(a, b) { return a + b }
 * ->
 * const add = (a, b) => a + b
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.FunctionDeclaration)
        .forEach((path) => {
            if (j.MethodDefinition.check(path.parent.node)) return
            if (j.Property.check(path.parent.node)) return
            if (j.ExportDeclaration.check(path.parentPath.node)) return
            if (j.ExportDefaultDeclaration.check(path.parent.node)) return
            if (j(path.node).find(j.ThisExpression).size() > 0) return

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

function createArrowFunctionExpression(j: JSCodeshift, fn: FunctionDeclaration) {
    const { params, body, async, comments } = fn
    const arrowFunction = j.arrowFunctionExpression(
        params,
        body,
        false,
    )
    arrowFunction.async = async
    arrowFunction.comments = comments
    return arrowFunction
}

export default wrap(transformAST)
