import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { ConditionalExpression, IfStatement } from 'jscodeshift'

/**
 * Unwraps nested ternary expressions into if-else statements.
 * Conditionally returns early if possible.
 *
 * @example
 * `a ? b() : c ? d() : e()`
 * ->
 * if(a) { b() }
 * if(c) { d() }
 * e()
 *
 * `return x ? a() : b()` -> `if (x) { return a() } else { return b() }`
 * `return x && a()` -> `if (x) { return a() }`
 * `return x || a()` -> `if (!x) { return a() }`
 * `return x ?? a()` -> `if (x == null) { return a() }`
 *
 * `x ? a() : b()` -> `if (x) { a() } else { b() }`
 * `x && a()` -> `if (x) { a() }`
 * `x || a()` -> `if (!x) { a() }`
 * `x ?? a()` -> `if (x == null) { a() }`
 *
 * @see https://babeljs.io/docs/en/babel-plugin-minify-simplify#reduce-statement-into-expression
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context
    const NullIdentifier = j.identifier('null')

    /**
     * Nested ternary expression
     *
     * we can only confidently transform the nested ternary
     * expression under ExpressionStatement.
     * use "Early return" to avoid deeply nested if statement
     * `a ? b() : c ? d() : e()`
     * ->
     * if(a) { b() }
     * if(c) { d() }
     * e()
     */
    while (true) {
        const conditionExpressionNodes = root.find(j.ConditionalExpression, (node) => {
            return node.consequent.type === 'ConditionalExpression' || node.alternate.type === 'ConditionalExpression'
        })
        if (!conditionExpressionNodes.size()) break

        const path = conditionExpressionNodes.paths()[0]
        if (path.parentPath.node.type !== 'ExpressionStatement') break

        const prepend: IfStatement[] = []
        let tail: any = null
        const deNested = (conditionExpressionNode: ConditionalExpression) => {
            const ifStatementNode = j.ifStatement(
                conditionExpressionNode.test,
                j.blockStatement([j.expressionStatement(conditionExpressionNode.consequent)]),
            )
            prepend.push(ifStatementNode)
            if (j.ConditionalExpression.check(conditionExpressionNode.alternate)) {
                deNested(conditionExpressionNode.alternate)
            }
            else {
                tail = conditionExpressionNode.alternate
            }
        }
        deNested(path.node)
        j(path).closest(j.ExpressionStatement).insertBefore(prepend)
        if (tail) j(path).replaceWith(tail)
        else j(path).remove()
    }

    /**
     * Nested logical expression
     *
     * x == 'a' || x == 'b' || x == 'c' && x == 'd'
     */

    /**
     * Return simple ternary expression
     *
     * `return x ? a() : b()` -> `if (x) { return a() } else { return b() }`
     */
    root
        .find(j.ReturnStatement, {
            argument: {
                type: 'ConditionalExpression',
            },
        })
        .forEach((path) => {
            const { node } = path
            if (!j.ConditionalExpression.check(node.argument)) return

            const { test, consequent, alternate } = node.argument
            const consequentReturn = j.blockStatement([j.returnStatement(consequent)])
            const alternateReturn = j.blockStatement([j.returnStatement(alternate)])
            const replacement = j.ifStatement(test, consequentReturn, alternateReturn)
            j(path).replaceWith(replacement)
        })

    /**
     * Return simple logical expression
     * `return x && a()` -> `if (x) { return a() }`
     * `return x || a()` -> `if (!x) { return a() }`
     * `return x ?? a()` -> `if (x == null) { return a() }`
     */
    root
        .find(j.ReturnStatement, {
            argument: {
                type: 'LogicalExpression',
                operator: (operator: string) => ['&&', '||', '??'].includes(operator),
            },
        })
        .forEach((path) => {
            const { node } = path
            if (!j.LogicalExpression.check(node.argument)) return

            const { operator, left, right } = node.argument
            const test = operator === '&&'
                ? left
                : operator === '||'
                    ? j.unaryExpression('!', left)
                    : j.binaryExpression('==', left, NullIdentifier)
            const consequent = j.blockStatement([j.returnStatement(right)])
            const alternate = null
            const replacement = j.ifStatement(test, consequent, alternate)
            j(path).replaceWith(replacement)
        })

    /**
     * Simple ternary expression
     *
     * `x ? a() : b()` -> `if (x) { a() } else { b() }`
     */
    root
        .find(j.ExpressionStatement)
        .filter((path) => {
            const { node } = path
            return j.ConditionalExpression.check(node.expression)
        })
        .forEach((path) => {
            if (j.IfStatement.check(path.parentPath.node)) return
            if (j.LogicalExpression.check(path.parentPath.node)) return
            if (j.SequenceExpression.check(path.parentPath.node)) return
            if (j.VariableDeclarator.check(path.parentPath.node)) return
            if (j.AssignmentExpression.check(path.parentPath.node)) return
            if (j.ArrowFunctionExpression.check(path.parentPath.node)) return

            const { node } = path
            if (!j.ConditionalExpression.check(node.expression)) return

            const { test, consequent, alternate } = node.expression
            const consequentStatement = j.blockStatement([j.expressionStatement(consequent)])
            const alternateStatement = j.blockStatement([j.expressionStatement(alternate)])
            const replacement = j.ifStatement(test, consequentStatement, alternateStatement)
            j(path).replaceWith(replacement)
        })

    /**
     * Simple logical expression
     *
     * `x && a()` -> `if (x) { a() }`
     * `x || a()` -> `if (!x) { a() }`
     * `x ?? a()` -> `if (x == null) { a() }`
     */
    root
        .find(j.LogicalExpression, {
            operator: (operator: string) => ['&&', '||', '??'].includes(operator),
        })
        .forEach((path) => {
            if (j.IfStatement.check(path.parentPath.node)) return
            if (j.LogicalExpression.check(path.parentPath.node)) return
            // TODO: need to come up with a better way to handle LogicalExpression in SequenceExpression
            if (j.SequenceExpression.check(path.parentPath.node)) return
            if (j.VariableDeclarator.check(path.parentPath.node)) return
            if (j.AssignmentExpression.check(path.parentPath.node)) return
            if (j.ConditionalExpression.check(path.parentPath.node)) return
            if (j.ArrowFunctionExpression.check(path.parentPath.node)) return

            const { node } = path
            const { operator, left, right } = node
            if (j.LogicalExpression.check(left) || j.LogicalExpression.check(right)) return

            const test = operator === '&&'
                ? left
                : operator === '||'
                    ? j.unaryExpression('!', left)
                    : j.binaryExpression('==', left, NullIdentifier)
            const consequent = j.blockStatement([j.expressionStatement(right)])
            const alternate = null
            const replacement = j.ifStatement(test, consequent, alternate)
            j(path).replaceWith(replacement)
        })
}

export default wrap(transformAST)
