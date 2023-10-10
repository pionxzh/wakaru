import { isUndefined } from '../utils/checker'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Simplify the last return statements.
 *
 * The following patterns will be removed:
 * - `return undefined`
 * - `return void 0`
 *
 * @example
 * return void a()
 * ->
 * a();
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.ReturnStatement)
        .filter((path) => {
            const isBlockParent = j.BlockStatement.check(path.parent.node)
            const parent = isBlockParent ? path.parent.parent.node : path.parent.node

            if (
                j.FunctionDeclaration.check(parent)
                || j.FunctionExpression.check(parent)
                || j.ArrowFunctionExpression.check(parent)
                || j.MethodDefinition.check(parent)
                || j.ObjectMethod.check(parent)
                || j.ClassMethod.check(parent)
            ) {
                // @ts-expect-error cannot guard this type
                const body = isBlockParent ? parent.body.body : parent.body
                return body[body.length - 1] === path.node
            }

            return false
        })
        .forEach((path) => {
            const argument = path.node.argument
            if (!argument || isUndefined(j, argument)) {
                path.prune()
                return
            }

            if (j.UnaryExpression.check(argument) && argument.operator === 'void') {
                path.replace(j.expressionStatement(argument.argument))
            }
        })
}

export default wrap(transformAST)
