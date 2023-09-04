import picocolors from 'picocolors'
import { makeDecisionTree, makeDecisionTreeWithConditionSplitting } from '../utils/decisionTree'
import { markParenthesized } from '../utils/parenthesized'
import wrap from '../wrapAstTransformation'
import type { DecisionTree } from '../utils/decisionTree'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { ASTNode, BinaryExpression, ConditionalExpression, Identifier, JSCodeshift, LogicalExpression } from 'jscodeshift'

/**
 * Restore optional chaining syntax.
 *
 * Only support `loose=false` mode.
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.ConditionalExpression)
        .forEach((path) => {
            const result = convertOptionalChaining(j, path.node)
            if (result) {
                path.replace(result)
            }
        })

    root
        .find(j.LogicalExpression, { operator: '||' })
        .forEach((path) => {
            const result = convertOptionalChaining(j, path.node)
            if (result) {
                path.replace(result)
            }
        })
}

function convertOptionalChaining(j: JSCodeshift, expression: ConditionalExpression | LogicalExpression): ExpressionKind | null {
    const _decisionTree = makeDecisionTree(j, expression)
    const decisionTree = makeDecisionTreeWithConditionSplitting(j, _decisionTree)
    // renderDebugDecisionTree(j, decisionTree)
    const result = constructOptionalChaining(j, decisionTree)
    if (result && result !== decisionTree.condition) {
        return result
    }
    return null
}

function constructOptionalChaining(j: JSCodeshift, tree: DecisionTree, flag = 0): ExpressionKind | null {
    const { condition, trueBranch, falseBranch } = tree

    if (!isFalsyBranch(j, trueBranch)) return null

    /**
     * Flag 0: Default state, looking for null
     * Flag 1: Null detected, looking for undefined
     */
    if (flag === 0) {
        if (!falseBranch) return condition

        if (isNullBinary(j, condition)) {
            const { left, right } = condition
            const cond = constructOptionalChaining(j, falseBranch, 1)
            if (!cond) return null
            if (j.AssignmentExpression.check(left) && j.Identifier.check(left.left)) {
                const nestedAssignment = j(left).find(j.AssignmentExpression, { left: { type: 'Identifier' } }).nodes()
                const allAssignment = [left, ...nestedAssignment]
                const result = allAssignment.reduce((acc, curr) => {
                    const { left: tempVariable, right: originalVariable } = curr

                    return applyOptionalChaining(j, acc, tempVariable as Identifier, originalVariable)
                }, cond)
                return result
            }
            else if (j.Identifier.check(left)) {
                return applyOptionalChaining(j, cond, left, undefined)
            }
        }
    }
    else if (flag === 1) {
        if (!falseBranch) return null

        if (isUndefinedBinary(j, condition)) {
            return constructOptionalChaining(j, falseBranch, 0)
        }
        return null
    }

    return null
}

function applyOptionalChaining<T extends ExpressionKind>(
    j: JSCodeshift,
    node: T,
    tempId?: Identifier,
    targetExpression?: ExpressionKind,
): T {
    console.log('applyOptionalChaining', j(node).toSource())
    if (j.MemberExpression.check(node)) {
        if (j.Identifier.check(node.object)
        && tempId
        && node.object.name === tempId.name
        ) {
            /**
             * Wrap with parenthesis to ensure the precedence.
             * The output will be a little bit ugly, but it
             * will eventually be cleaned up by prettier.
             */
            const object = targetExpression
                ? markParenthesized(targetExpression, true)
                : node.object
            return j.optionalMemberExpression(object, node.property) as T
        }

        node.object = applyOptionalChaining(j, node.object, tempId, targetExpression)
    }

    if ((j.CallExpression.check(node) || j.OptionalCallExpression.check(node))) {
        if ((j.MemberExpression.check(node.callee) || j.OptionalMemberExpression.check(node.callee))) {
            if (j.MemberExpression.check(node.callee.object)
                && j.Identifier.check(node.callee.property)
                && node.callee.property.name === 'call'
                && tempId
                && j.Identifier.check(node.arguments[0])
                && node.arguments[0].name === tempId.name
            ) {
                const argumentStartsWithThis = tempId
                    && j.Identifier.check(node.arguments[0])
                    && node.arguments[0].name === tempId.name
                const [_, ..._args] = node.arguments
                const args = argumentStartsWithThis ? _args : node.arguments
                const callee = node.callee
                const optionalCallExpression = j.optionalCallExpression(callee.object as Identifier, args)
                optionalCallExpression.callee = applyOptionalChaining(j, optionalCallExpression.callee, tempId, targetExpression)
                optionalCallExpression.arguments = optionalCallExpression.arguments.map((arg) => {
                    return j.SpreadElement.check(arg) ? arg : applyOptionalChaining(j, arg, tempId, targetExpression)
                })
                return optionalCallExpression as T
            }
        }

        if (j.Identifier.check(node.callee) && tempId && targetExpression && node.callee.name === tempId.name) {
            return j.optionalCallExpression(targetExpression, node.arguments) as T
        }

        node.callee = applyOptionalChaining(j, node.callee, tempId, targetExpression)
        node.arguments = node.arguments.map((arg) => {
            return j.SpreadElement.check(arg) ? arg : applyOptionalChaining(j, arg, tempId, targetExpression)
        })
    }

    if (j.AssignmentExpression.check(node)) {
        if (j.Identifier.check(node.left) && tempId && node.left.name === tempId.name && targetExpression) {
            if (node.right === targetExpression) {
                return targetExpression as T
            }
            return j.assignmentExpression(node.operator, targetExpression, node.right) as T
        }
    }

    if (j.Identifier.check(node)) {
        if (tempId && node.name === tempId.name && targetExpression) {
            return targetExpression as T
        }
    }

    return node
}

function isFalsyBranch(j: JSCodeshift, tree: DecisionTree | null): boolean {
    if (!tree) return true

    const { condition, trueBranch, falseBranch } = tree

    return (isNull(j, condition) || isUndefined(j, condition))
        && (!trueBranch || isFalsyBranch(j, trueBranch))
        && (!falseBranch || isFalsyBranch(j, falseBranch))
}

function isNullBinary(j: JSCodeshift, node: ASTNode): node is BinaryExpression {
    return j.BinaryExpression.check(node)
    && node.operator === '==='
    && (isNull(j, node.left) || isNull(j, node.right))
}

function isUndefinedBinary(j: JSCodeshift, node: ASTNode): node is BinaryExpression {
    return j.BinaryExpression.check(node)
    && node.operator === '==='
    && (isUndefined(j, node.left) || isUndefined(j, node.right))
}

function isNull(j: JSCodeshift, node: ASTNode) {
    return j.Literal.check(node) && node.value === null
}

function isUndefined(j: JSCodeshift, node: ASTNode) {
    return isVoid0(j, node)
    || (j.Identifier.check(node) && node.name === 'undefined')
}

function isVoid0(j: JSCodeshift, node: ASTNode) {
    return j.UnaryExpression.check(node) && node.operator === 'void' && j.Literal.check(node.argument) && node.argument.value === 0
}

export default wrap(transformAST)
