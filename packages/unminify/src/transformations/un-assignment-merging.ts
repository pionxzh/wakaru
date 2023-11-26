import { mergeComments } from '@wakaru/ast-utils'
import { isSimpleValue } from '../utils/checker'
import { replaceWithMultipleStatements } from '../utils/insert'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { AssignmentExpression } from 'jscodeshift'

/**
 * Separate chained assignment into multiple statements.
 * This rule is only applied to simple value assignment to
 * avoid introducing behavior changes.
 *
 * Normally, this rule should assign the next variable to the
 * previous one, which is also how the code is executed.
 *
 * For example:
 * ```js
 * exports.foo = exports.bar = 1
 * -> should be
 * exports.bar = 1
 * exports.foo = exports.bar
 * ```
 *
 * But instead, in this rule, it is assigned to the original value
 * to maximize the readability, and ease some edge cases that other
 * rules might hit on.
 *
 * @example
 * exports.foo = exports.bar = 1
 * ->
 * exports.bar = 1
 * exports.foo = 1
 *
 * @example
 * foo = bar = baz = void 0
 * ->
 * foo = void 0
 * bar = void 0
 * baz = void 0
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.ExpressionStatement, {
            expression: {
                type: 'AssignmentExpression',
                operator: '=',
                right: {
                    type: 'AssignmentExpression',
                    operator: '=',
                },
            },
        })
        .forEach((p) => {
            const { expression } = p.node

            let node = expression as AssignmentExpression
            const assignments: AssignmentExpression[] = [node]
            while (j.AssignmentExpression.check(node.right)) {
                node = node.right
                assignments.push(node)
            }

            if (assignments.length < 2) return

            const valueNode = node.right
            if (
                j.Identifier.check(valueNode)
                || isSimpleValue(j, valueNode)
            ) {
                const replacements = assignments.map((assignment) => {
                    return j.expressionStatement(j.assignmentExpression('=', assignment.left, valueNode))
                })
                mergeComments(replacements, p.node.comments)

                replaceWithMultipleStatements(j, p, replacements)
            }
        })
}

export default wrap(transformAST)
