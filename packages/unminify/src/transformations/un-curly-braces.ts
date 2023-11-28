import { wrapAstTransformation } from '@wakaru/ast-utils'
import type { ASTTransformation } from '@wakaru/ast-utils'
import type { JSCodeshift, VariableDeclaration } from 'jscodeshift'

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
            if (!j.BlockStatement.check(path.value.consequent) && !isVarDeclaration(j, path.value.consequent)) {
                path.value.consequent = j.blockStatement([path.value.consequent])
            }

            if (path.value.alternate
            && !j.BlockStatement.check(path.value.alternate)
            && !isVarDeclaration(j, path.value.alternate)
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
        // j.LabeledStatement, // Adding BlockStatement to LabeledStatement is problematic
        // j.WithStatement, // parser will die bcz of strict mode
    ]

    nodesWithBody.forEach((node) => {
        // @ts-expect-error
        root.find(node, {
            body: value => !j.BlockStatement.check(value)
            && !isVarDeclaration(j, value),
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
                return value.length > 0 && !j.BlockStatement.check(value[0])
            },
        })
        .forEach((path) => {
            path.value.consequent = [j.blockStatement(path.value.consequent)]
        })
}

/**
 * Check if the node is a `VariableDeclaration` with `kind` equals to `var`.
 *
 * We avoid wrapping var declaration with `BlockStatement` because it will
 * change the scope of the variable.
 * See https://github.com/lebab/lebab/pull/348 for more details.
 */
function isVarDeclaration(j: JSCodeshift, node: any): node is VariableDeclaration {
    return j.VariableDeclaration.check(node) && node.kind === 'var'
}

export default wrapAstTransformation(transformAST)
