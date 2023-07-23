import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * `a(), b(), c()` -> `a(); b(); c();`
 * `return a(), b()` -> `a(); return b()`
 *
 * @see https://babeljs.io/docs/en/babel-helper-to-multiple-sequence-expressions
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    // `return a(), b()` -> `a(); return b()`
    root
        .find(j.ReturnStatement, {
            argument: {
                type: 'SequenceExpression',
            },
        })
        .forEach((path) => {
            const { node: { argument } } = path
            if (!j.SequenceExpression.check(argument)) return

            const { expressions } = argument
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            if (j.AssignmentExpression.check(last) && j.Identifier.check(last.left)) {
                replacement.push(j.expressionStatement(last))
                replacement.push(j.returnStatement(j.identifier(last.left.name)))
            }
            else {
                replacement.push(j.returnStatement(last))
            }

            if (j.IfStatement.check(path.parentPath.node)) {
                path.parentPath.replace(j.blockStatement(replacement))
            }
            else {
                j(path).replaceWith(replacement)
            }
        })

    // `a(), b(), c()` -> `a(); b(); c();`
    root
        .find(j.ExpressionStatement, {
            expression: {
                type: 'SequenceExpression',
            },
        })
        .forEach((path) => {
            const { node: { expression } } = path
            if (!j.SequenceExpression.check(expression)) return
            if (j.IfStatement.check(path.parent.node)) return
            if (j.ForStatement.check(path.parent.node)) return

            const { expressions } = expression
            const replacement = expressions.map(e => j.expressionStatement(e))
            // console.log(j(replacement).toSource())
            j(path).replaceWith(replacement)
        })
}

export default wrap(transformAST)
