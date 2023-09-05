import { negateCondition } from './negateCondition'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { JSCodeshift } from 'jscodeshift'

export interface DecisionTree {
    condition: ExpressionKind
    trueBranch: DecisionTree | null
    falseBranch: DecisionTree | null
}

export function makeDecisionTree(j: JSCodeshift, node: ExpressionKind): DecisionTree {
    if (j.ConditionalExpression.check(node)) {
        return {
            condition: node.test,
            trueBranch: makeDecisionTree(j, node.consequent),
            falseBranch: makeDecisionTree(j, node.alternate),
        }
    }

    if (j.LogicalExpression.check(node)) {
        if (node.operator === '&&') {
            return {
                condition: node.left,
                trueBranch: makeDecisionTree(j, node.right),
                falseBranch: null,
            }
        }

        if (node.operator === '||') {
            return {
                condition: node.left,
                trueBranch: null,
                falseBranch: makeDecisionTree(j, node.right),
            }
        }

        // if (node.operator === '??') {
        //     return {
        //         condition: j.binaryExpression('==', node.left, j.identifier('null')),
        //         trueBranch: makeDecisionTree(j, node.right),
        //         falseBranch: null,
        //     }
        // }
    }

    return {
        condition: node,
        trueBranch: null,
        falseBranch: null,
    }
}

export function negateDecisionTree(j: JSCodeshift, tree: DecisionTree): DecisionTree {
    const { condition, trueBranch, falseBranch } = tree

    return {
        condition: negateCondition(j, condition),
        trueBranch: falseBranch ? negateDecisionTree(j, falseBranch) : null,
        falseBranch: trueBranch ? negateDecisionTree(j, trueBranch) : null,
    }
}

export function makeDecisionTreeWithConditionSplitting(j: JSCodeshift, tree: DecisionTree): DecisionTree {
    const { condition, trueBranch, falseBranch } = tree

    if (j.LogicalExpression.check(condition)) {
        if (condition.operator === '&&') {
            return makeDecisionTreeWithConditionSplitting(j, {
                condition: condition.left,
                trueBranch: makeDecisionTreeWithConditionSplitting(j, {
                    condition: condition.right,
                    trueBranch,
                    falseBranch,
                }),
                falseBranch: null,
            })
        }

        if (condition.operator === '||') {
            return makeDecisionTreeWithConditionSplitting(j, {
                condition: condition.left,
                trueBranch: null,
                falseBranch: makeDecisionTreeWithConditionSplitting(j, {
                    condition: condition.right,
                    trueBranch,
                    falseBranch,
                }),
            })
        }
    }

    return {
        condition,
        trueBranch: trueBranch ? makeDecisionTreeWithConditionSplitting(j, trueBranch) : null,
        falseBranch: falseBranch ? makeDecisionTreeWithConditionSplitting(j, falseBranch) : null,
    }
}

export function isDecisionTreeLeaf(tree: DecisionTree): boolean {
    return tree.trueBranch === null && tree.falseBranch === null
}

export function renderDebugDecisionTree(j: JSCodeshift, tree: DecisionTree) {
    const output = JSON.stringify(tree, (_, value) => {
        if (value === null) return null
        if ('type' in value) return j(value).toSource()
        return value
    }, 2)
    console.log(output)
}
