import { mergeComments } from '@wakaru/ast-utils/comments'
import { areNodesEqual, isNotNullBinary, isNull, isNullBinary, isTrue, isUndefined, isUndefinedBinary } from '@wakaru/ast-utils/matchers'
import { smartParenthesized } from '@wakaru/ast-utils/parenthesized'
import { removeDeclarationIfUnused } from '@wakaru/ast-utils/scope'
import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import { negateCondition } from '../utils/condition'
import { makeDecisionTree, makeDecisionTreeWithConditionSplitting, negateDecisionTree } from '../utils/decisionTree'
import type { DecisionTree } from '../utils/decisionTree'
import type { ASTTransformation } from '@wakaru/shared/rule'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { ASTPath, ConditionalExpression, Identifier, JSCodeshift, LogicalExpression, MemberExpression, SequenceExpression, SpreadElement } from 'jscodeshift'

/**
 * Restore optional chaining syntax.
 *
 * TODO: support `loose=false` mode.
 * if (foo != null && foo.length > 0) -> if (foo?.length > 0)
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    const visited = new Set<ASTPath>()

    let passes = 5
    while (passes--) {
        root
            .find(j.ConditionalExpression)
            .forEach((path) => {
                if (visited.has(path)) return
                visited.add(path)

                const result = convertOptionalChaining(j, path)
                if (result) {
                    path.replace(result)
                }
            })

        root
            .find(j.LogicalExpression, { operator: (op: LogicalExpression['operator']) => op === '&&' || op === '||' })
            .forEach((path) => {
                if (visited.has(path)) return
                visited.add(path)

                const result = convertOptionalChaining(j, path)
                if (result) {
                    path.replace(result)
                }
            })
    }
}

function convertOptionalChaining(j: JSCodeshift, path: ASTPath<ConditionalExpression | LogicalExpression>): ExpressionKind | null {
    const expression = path.node
    // console.log('\n\n>>>', `${picocolors.green(j(expression).toSource())}`)
    const _decisionTree = makeDecisionTreeWithConditionSplitting(j, makeDecisionTree(j, expression))
    const isNotNull = isNotNullBinary(j, _decisionTree.condition)
    const decisionTree = isNotNull
        ? negateDecisionTree(j, _decisionTree)
        : _decisionTree
    // renderDebugDecisionTree(j, decisionTree)

    const _result = constructOptionalChaining(j, path, decisionTree, 0)
    const result = _result && isNotNull ? negateCondition(j, _result) : _result
    if (result) {
        // console.log('<<<', `${picocolors.cyan(j(result).toSource())}`)
        mergeComments(result, expression.comments)
    }
    return result
}

function constructOptionalChaining(
    j: JSCodeshift,
    path: ASTPath,
    tree: DecisionTree,
    flag: 0 | 1,
): ExpressionKind | null {
    const { condition, trueBranch, falseBranch } = tree

    const deepestFalseBranch = getDeepestFalseBranch(tree)
    /**
     * if the deepest node is `delete` operator, we should accept true as the
     * condition.
     * @see https://github.com/babel/babel/blob/aaf364a5675daec4dc61095c5fd6df6c9adf71cf/packages/babel-plugin-transform-optional-chaining/src/transform.ts#L251
     */
    if (trueBranch && j.UnaryExpression.check(deepestFalseBranch.condition) && deepestFalseBranch.condition.operator === 'delete') {
        if (!isFalsyBranch(j, trueBranch) && !isTrue(j, trueBranch.condition)) return null
    }
    else if (!isFalsyBranch(j, trueBranch)) return null

    /**
     * Flag 0: Default state, looking for null
     * Flag 1: Null detected, looking for undefined
     */
    if (flag === 0) {
        if (!falseBranch) {
            const nestedAssignment = j(condition).find(j.AssignmentExpression, { left: { type: 'Identifier' } }).nodes()

            const allAssignment = [
                ...nestedAssignment,
                ...(j.AssignmentExpression.check(condition) && j.Identifier.check(condition.left) ? [condition] : []),
            ]
            const result = allAssignment.reduce((acc, curr) => {
                const { left: tempVariable, right: originalVariable } = curr

                return applyOptionalChaining(j, acc, tempVariable as Identifier, originalVariable)
            }, condition)

            allAssignment.forEach((assignment) => {
                if (j.Identifier.check(assignment.left)) {
                    removeDeclarationIfUnused(j, path, assignment.left.name)
                }
            })

            return result
        }

        if (isNullBinary(j, condition)) {
            const { left, right: _ } = condition
            const cond = constructOptionalChaining(j, path, falseBranch, 1)
            if (!cond) return null
            if (j.AssignmentExpression.check(left) && j.Identifier.check(left.left)) {
                const nestedAssignment = j(left).find(j.AssignmentExpression, { left: { type: 'Identifier' } }).nodes()
                const allAssignment = [left, ...nestedAssignment]
                const result = allAssignment.reduce((acc, curr) => {
                    const { left: tempVariable, right: originalVariable } = curr

                    return applyOptionalChaining(j, acc, tempVariable as Identifier, originalVariable)
                }, cond)

                allAssignment.forEach((assignment) => {
                    if (j.Identifier.check(assignment.left)) {
                        removeDeclarationIfUnused(j, path, assignment.left.name)
                    }
                })

                return result
            }
            else if (j.Identifier.check(left)) {
                return applyOptionalChaining(j, cond, left, undefined)
            }
            else if (j.MemberExpression.check(left)) {
                return applyOptionalChaining(j, cond, left, undefined)
            }
        }
    }
    else if (flag === 1) {
        if (!falseBranch) return null

        if (isUndefinedBinary(j, condition)) {
            return constructOptionalChaining(j, path, falseBranch, 0)
        }
        return null
    }

    return null
}

function applyOptionalChaining<T extends ExpressionKind>(
    j: JSCodeshift,
    node: T,
    tempVariable: MemberExpression | Identifier,
    targetExpression?: ExpressionKind,
): T {
    // console.log('applyOptionalChaining', node.type, j(node).toSource(), '|', tempVariable ? j(tempVariable).toSource() : null, '|', targetExpression ? j(targetExpression).toSource() : null)

    if (j.MemberExpression.check(node)) {
        if (areNodesEqual(j, node.object, tempVariable)) {
            /**
             * Wrap with parenthesis to ensure the precedence.
             * The output will be a little bit ugly, but it
             * will eventually be cleaned up by prettier.
             */
            const object = targetExpression
                ? smartParenthesized(j, targetExpression)
                : node.object
            return j.optionalMemberExpression(object, node.property, node.computed) as T
        }

        node.object = applyOptionalChaining(j, node.object, tempVariable, targetExpression)
    }

    if ((j.CallExpression.check(node) || j.OptionalCallExpression.check(node))) {
        if ((j.MemberExpression.check(node.callee) || j.OptionalMemberExpression.check(node.callee))) {
            if (j.MemberExpression.check(node.callee.object)
                && j.Identifier.check(node.callee.property)
            ) {
                if (
                    node.callee.property.name === 'call'
                    && areNodesEqual(j, node.arguments[0], tempVariable)
                ) {
                    const argumentStartsWithThis = areNodesEqual(j, node.arguments[0], tempVariable)
                    const [_, ..._args] = node.arguments
                    const args = argumentStartsWithThis ? _args : node.arguments
                    const callee = node.callee
                    const optionalCallExpression = j.optionalCallExpression(callee.object as Identifier, args)
                    optionalCallExpression.callee = applyOptionalChaining(j, optionalCallExpression.callee, tempVariable, targetExpression)
                    optionalCallExpression.arguments = optionalCallExpression.arguments.map((arg) => {
                        return j.SpreadElement.check(arg) ? arg : applyOptionalChaining(j, arg, tempVariable, targetExpression)
                    })
                    return optionalCallExpression as T
                }

                if (node.callee.property.name === 'apply') {
                    const [_, arg] = node.arguments
                    if (j.SpreadElement.check(arg)) return node

                    const args = j.ArrayExpression.check(arg)
                        ? arg.elements.map(element => element ?? j.identifier('undefined')) as Array<ExpressionKind | SpreadElement>
                        : [j.spreadElement(arg)]
                    const callee = node.callee
                    const optionalCallExpression = j.optionalCallExpression(callee.object as Identifier, args)
                    optionalCallExpression.callee = applyOptionalChaining(j, optionalCallExpression.callee, tempVariable, targetExpression)
                    optionalCallExpression.arguments = optionalCallExpression.arguments.map((arg) => {
                        return j.SpreadElement.check(arg) ? arg : applyOptionalChaining(j, arg, tempVariable, targetExpression)
                    })
                    return optionalCallExpression as T
                }

                if (
                    node.callee.property.name === 'bind'
                    && areNodesEqual(j, node.arguments[0], tempVariable)
                ) {
                    const calleeObj = node.callee.object
                    const isOptional = !j.AssignmentExpression.check(calleeObj.object)
                    const memberExpression = isOptional
                        ? j.optionalMemberExpression(calleeObj.object, calleeObj.property, calleeObj.computed)
                        : j.memberExpression(calleeObj.object, calleeObj.property, calleeObj.computed)
                    memberExpression.object = applyOptionalChaining(j, memberExpression.object, tempVariable, targetExpression)
                    memberExpression.property = applyOptionalChaining(j, memberExpression.property, tempVariable, targetExpression)
                    return memberExpression as T
                }
            }

            if (areNodesEqual(j, node.callee.object, tempVariable)) {
                if (j.Identifier.check(node.callee.property)) {
                    if (node.callee.property.name === 'call') {
                        const optionalCallExpression = j.optionalCallExpression(targetExpression as Identifier, node.arguments)
                        optionalCallExpression.callee = applyOptionalChaining(j, optionalCallExpression.callee, tempVariable, targetExpression)
                        optionalCallExpression.arguments = optionalCallExpression.arguments.map((arg) => {
                            return j.SpreadElement.check(arg) ? arg : applyOptionalChaining(j, arg, tempVariable, targetExpression)
                        }).splice(1)
                        return optionalCallExpression as T
                    }
                    else if (node.callee.property.name === 'apply') {
                        const [_, arg] = node.arguments
                        if (j.SpreadElement.check(arg)) return node

                        const args = j.ArrayExpression.check(arg)
                            ? arg.elements.map(element => element ?? j.identifier('undefined')) as Array<ExpressionKind | SpreadElement>
                            : [j.spreadElement(arg)]
                        const optionalCallExpression = j.optionalCallExpression(targetExpression as Identifier, args)
                        optionalCallExpression.callee = applyOptionalChaining(j, optionalCallExpression.callee, tempVariable, targetExpression)
                        optionalCallExpression.arguments = optionalCallExpression.arguments.map((arg) => {
                            return j.SpreadElement.check(arg) ? arg : applyOptionalChaining(j, arg, tempVariable, targetExpression)
                        })
                        return optionalCallExpression as T
                    }
                }
            }
        }

        if (j.match(node.callee, {
            type: 'SequenceExpression',
            // @ts-expect-error
            expressions: (expressions: ExpressionKind[]) => {
                return expressions.length === 2
                && j.NumericLiteral.check(expressions[0])
                && expressions[0].value === 0
                && areNodesEqual(j, expressions[1], tempVariable)
            },
        })) {
            const target = targetExpression || (node.callee as SequenceExpression).expressions[1]
            const callee = smartParenthesized(j, j.sequenceExpression([j.numericLiteral(0), target]))
            const optionalCallExpression = j.optionalCallExpression(callee, node.arguments)
            optionalCallExpression.arguments = optionalCallExpression.arguments.map((arg) => {
                return j.SpreadElement.check(arg) ? arg : applyOptionalChaining(j, arg, tempVariable, targetExpression)
            })
            return optionalCallExpression as T
        }

        if (areNodesEqual(j, node.callee, tempVariable)) {
            const target = targetExpression || node.callee
            return j.optionalCallExpression(target, node.arguments) as T
        }

        node.callee = applyOptionalChaining(j, node.callee, tempVariable, targetExpression)
        node.arguments = node.arguments.map((arg) => {
            return j.SpreadElement.check(arg) ? arg : applyOptionalChaining(j, arg, tempVariable, targetExpression)
        })
    }

    if (j.AssignmentExpression.check(node)) {
        if (areNodesEqual(j, node.left, tempVariable) && targetExpression) {
            if (node.right === targetExpression) {
                return targetExpression as T
            }
            node.left = targetExpression as any
        }
    }

    if (j.Identifier.check(node) && areNodesEqual(j, node, tempVariable) && targetExpression) {
        return smartParenthesized(j, targetExpression) as T
    }

    if (j.UnaryExpression.check(node)) {
        node.argument = applyOptionalChaining(j, node.argument, tempVariable, targetExpression)
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

function getDeepestFalseBranch(tree: DecisionTree) {
    const { falseBranch } = tree
    if (!falseBranch) return tree

    return getDeepestFalseBranch(falseBranch)
}

export default createJSCodeshiftTransformationRule({
    name: 'un-optional-chaining',
    transform: transformAST,
})
