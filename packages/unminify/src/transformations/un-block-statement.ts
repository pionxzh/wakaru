import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Add `BlockStatement` to the following nodes:
 * - `IfStatement`
 * - `ForStatement`
 * - `ForInStatement`
 * - `ForOfStatement`
 * - `WhileStatement`
 * - `DoWhileStatement`
 * - `LabeledStatement`
 * - `ArrowFunctionExpression`
 * - `SwitchCase`
 *
 * Our many rules rely on having a `BlockStatement` to safely insert new nodes.
 * And this can potentially improve the readability.
 *
 * @example
 * for (let i = 0; i < 10; i++) console.log(i)
 * ->
 * for (let i = 0; i < 10; i++) { console.log(i) }
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.IfStatement)
        .forEach((path) => {
            if (!j.BlockStatement.check(path.value.consequent)) {
                path.value.consequent = j.blockStatement([path.value.consequent])
            }

            if (path.value.alternate
            && !j.BlockStatement.check(path.value.alternate)
            && !j.IfStatement.check(path.value.alternate)) {
                path.value.alternate = j.blockStatement([path.value.alternate])
            }
        })

    const nodesWithBody = [
        j.ForStatement,
        j.ForInStatement,
        j.ForOfStatement,
        j.WhileStatement,
        j.DoWhileStatement,
        // j.SwitchStatement,
        // j.TryStatement,
        // j.CatchClause, // CatchClause's body is always BlockStatement,
        // j.FunctionExpression, // FunctionExpression's body is always BlockStatement,
        // j.FunctionDeclaration, // FunctionDeclaration's body is always BlockStatement,
        j.LabeledStatement, // Adding BlockStatement to LabeledStatement is problematic
        // j.WithStatement, // parser will die bcz of strict mode
    ]

    nodesWithBody.forEach((node) => {
        // @ts-expect-error
        root.find(node, {
            body: value => value.type !== 'BlockStatement',
        }).forEach((path) => {
            path.value.body = j.blockStatement([path.value.body])
        })
    })

    root
        .find(j.ArrowFunctionExpression)
        .forEach((path) => {
            if (j.BlockStatement.check(path.value.body)) return
            path.value.body = j.blockStatement([j.returnStatement(path.value.body)])
        })

    root
        .find(j.SwitchCase, {
            consequent(value) {
                return value.length > 0 && value[0].type !== 'BlockStatement'
            },
        })
        .forEach((path) => {
            path.value.consequent = [j.blockStatement(path.value.consequent)]
        })
}

export default wrap(transformAST)
