import { areNodesEqual } from '../utils/checker'
import { negateCondition } from '../utils/condition'
import { makeDecisionTree } from '../utils/decisionTree'
import { replaceWithMultipleStatements } from '../utils/insert'
import wrap from '../wrapAstTransformation'
import type { DecisionTree } from '../utils/decisionTree'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { ExpressionKind, StatementKind } from 'ast-types/lib/gen/kinds'
import type { ASTNode, ConditionalExpression, JSCodeshift, LogicalExpression, SwitchCase } from 'jscodeshift'

/**
 * Unwraps nested ternary expressions and binary expression into if-else statements or switch statements.
 * Conditionally returns early if possible.
 *
 * @example
 * `a ? b() : c ? d() : e()`
 * ->
 * if (a) { b() }
 * else if (c) { d() }
 * else { e() }
 *
 * `return x ? a() : b()` -> `if (x) { return a() } return b()`
 * `return x && a()` -> `if (x) { return a() }`
 * `return x || a()` -> `if (!x) { return a() }`
 * `return x ?? a()` -> `if (x == null) { return a() } return x`
 *
 * `x ? a() : b()` -> `if (x) { a() } else { b() }`
 * `x && a()` -> `if (x) { a() }`
 * `x || a()` -> `if (!x) { a() }`
 * `x ?? a()` -> `if (x == null) { a() }`
 *
 * @example
 * foo == 'bar' ? bar() : foo == 'baz' ? baz() : foo == 'qux' || foo == 'quux' ? qux() : quux()
 * ->
 * switch (foo) {
 *  case 'bar':
 *   bar()
 *   break
 *  case 'baz':
 *   baz()
 *   break
 *  case 'qux':
 *  case 'quux':
 *   qux()
 *   break
 *  default:
 *   quux()
 * }
 *
 * @example
 * foo == 'bar' ? bar() : foo == 'baz' ? baz() : foo == 'qux' || foo == 'quux' && qux()
 * ->
 * switch (foo) {
 *  case 'bar':
 *   bar()
 *   break
 *  case 'baz':
 *   baz()
 *   break
 *  case 'qux':
 *  case 'quux':
 *   qux()
 *   break
 * }
 *
 * @see https://babeljs.io/docs/babel-plugin-minify-simplify#reduce-statement-into-expression
 * @see https://babeljs.io/docs/babel-plugin-minify-guarded-expressions
 * @see https://github.com/terser/terser/blob/master/test/compress/if_return.js
 * @see https://github.com/terser/terser/blob/master/test/compress/conditionals.js
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    /**
     * Nested ternary expression
     *
     * we can only confidently transform the nested ternary
     * expression under ExpressionStatement.
     *
     * @example
     * `a ? b() : c ? d() : e()`
     * ->
     * if (a) { b() }
     * else if (c) { d() }
     * else { e() }
     *
     * use "Early return" to avoid deeply nested if statements
     * when the nested ternary expression is under BlockStatement
     *
     * @example
     * `if (x) { return a ? b() : c ? d() : e() }`
     * ->
     * if (x) {
     *   if (a) { return b() }
     *   if (c) { return d() }
     *   return e()
     * }
     */
    root
        .find(j.ExpressionStatement, {
            /**
             * Expression with ternary operator will always be transformed
             */
            expression: {
                type: 'ConditionalExpression',
            },
        })
        .forEach((path) => {
            const conditionExpression = path.node.expression as ConditionalExpression
            const decisionTree = makeDecisionTree(j, conditionExpression)
            if (!shouldTransform(j, decisionTree)) return

            const replacements = renderDecisionTree(j, decisionTree)
            replaceWithMultipleStatements(j, path, replacements)
        })

    /**
     * Return nested ternary expression
     *
     * `return x ? a() : b()` -> `if (x) { return a() } else { return b() }`
     */
    root
        .find(j.ReturnStatement, {
            /**
             * Expression with ternary operator will only be transformed
             * if the ternary operator is nested
             *
             * Because `return a ? b() : c()` should be easier to read
             */
            argument: (node) => {
                if (j.ConditionalExpression.check(node)) {
                    return j.ConditionalExpression.check(node.consequent)
                        || j.ConditionalExpression.check(node.alternate)
                        || j.LogicalExpression.check(node.consequent)
                        || j.LogicalExpression.check(node.alternate)
                }

                return false
            },
        })
        .forEach((path) => {
            const conditionExpression = path.node.argument as ConditionalExpression
            const decisionTree = makeDecisionTree(j, conditionExpression)
            if (!shouldTransform(j, decisionTree)) return

            const replacements = renderDecisionTreeWithReturn(j, decisionTree)
            replaceWithMultipleStatements(j, path, replacements)
        })

    root
        .find(j.ExpressionStatement, {
            /**
             * Expression with logical expression will always be transformed
             */
            expression: {
                type: 'LogicalExpression',
            },
        })
        .forEach((path) => {
            const logicalExpression = path.node.expression as LogicalExpression
            const decisionTree = makeDecisionTree(j, logicalExpression)
            if (!shouldTransform(j, decisionTree)) return

            const replacements = renderDecisionTree(j, decisionTree)
            replaceWithMultipleStatements(j, path, replacements)
        })
}

function shouldTransform(j: JSCodeshift, tree: DecisionTree): boolean {
    const { condition, trueBranch, falseBranch } = tree

    if (!trueBranch && !falseBranch) {
        return !j.Identifier.check(condition)
        && !j.Literal.check(condition)
    }

    return (!trueBranch || shouldTransform(j, trueBranch))
    && (!falseBranch || shouldTransform(j, falseBranch))
}

function renderDecisionTree(j: JSCodeshift, tree: DecisionTree): StatementKind[] {
    return renderDecisionTreeToSwitch(j, tree)
        || renderDecisionTreeToIfElse(j, tree)
}

function renderDecisionTreeWithReturn(j: JSCodeshift, tree: DecisionTree): StatementKind[] {
    return renderDecisionTreeToSwitchWithReturn(j, tree)
        || renderDecisionTreeToIfElseWithReturn(j, tree)
}

/**
 * Renders a decision tree into if-else statements smartly.
 * Here is the comparison between the naive and smart approach:
 *
 * Naive approach:
 * ```js
 * if (a) {
 *   b()
 * }
 * else {
 *   if (c) {
 *     d()
 *   }
 *   else {
 *     e()
 *   }
 * }
 * ```
 *
 * Smart approach:
 * ```js
 * if (a) {
 *   b()
 * }
 * else if (c) {
 *   d()
 * }
 * else {
 *  e()
 * }
 * ```
 */
function renderDecisionTreeToIfElse(j: JSCodeshift, tree: DecisionTree): StatementKind[] {
    const { condition, trueBranch, falseBranch } = tree

    if (trueBranch && falseBranch) {
        const falseBranchStatements = renderDecisionTreeToIfElse(j, falseBranch)
        if (falseBranchStatements.length === 1 && j.IfStatement.check(falseBranchStatements[0])) {
            // generate an else-if statement
            return [
                j.ifStatement(
                    condition,
                    j.blockStatement(renderDecisionTreeToIfElse(j, trueBranch)),
                    falseBranchStatements[0],
                ),
            ]
        }
        else {
            // generate a nested if-else statement
            return [
                j.ifStatement(
                    condition,
                    j.blockStatement(renderDecisionTreeToIfElse(j, trueBranch)),
                    j.blockStatement(falseBranchStatements),
                ),
            ]
        }
    }

    if (trueBranch) {
        return [
            j.ifStatement(
                condition,
                j.blockStatement(renderDecisionTreeToIfElse(j, trueBranch)),
            ),
        ]
    }

    if (falseBranch) {
        return [
            j.ifStatement(
                negateCondition(j, condition),
                j.blockStatement(renderDecisionTreeToIfElse(j, falseBranch)),
            ),
        ]
    }

    return [j.expressionStatement(condition)]
}

function renderDecisionTreeToIfElseWithReturn(j: JSCodeshift, tree: DecisionTree): StatementKind[] {
    const { condition, trueBranch, falseBranch } = tree

    if (trueBranch && falseBranch) {
        const trueBranchStatements = renderDecisionTreeWithReturn(j, trueBranch)
        const falseBranchStatements = renderDecisionTreeWithReturn(j, falseBranch)

        return [
            j.ifStatement(
                condition,
                j.blockStatement(trueBranchStatements),
            ),
            ...falseBranchStatements,
        ]
    }

    if (trueBranch) {
        return [
            j.ifStatement(
                condition,
                j.blockStatement(renderDecisionTreeWithReturn(j, trueBranch)),
            ),
        ]
    }

    if (falseBranch) {
        return [
            j.ifStatement(
                j.unaryExpression('!', condition),
                j.blockStatement(renderDecisionTreeWithReturn(j, falseBranch)),
            ),
        ]
    }

    return condition ? [j.returnStatement(condition)] : []
}

const SWITCH_THRESHOLD = 3
function renderDecisionTreeToSwitch(j: JSCodeshift, tree: DecisionTree) {
    const cond = tree.condition
    const comparisonBases = extractComparisonBases(j, cond)
    if (comparisonBases.length === 0) return null

    const comparisonBase = comparisonBases.find(base => countBaseVariableUsedInAllBranches(j, base, tree) >= SWITCH_THRESHOLD)
    if (!comparisonBase) return null

    const switchCases = collectSwitchCase(j, tree, comparisonBase, false)
    return [j.switchStatement(comparisonBase, switchCases)]
}

function renderDecisionTreeToSwitchWithReturn(j: JSCodeshift, tree: DecisionTree) {
    const cond = tree.condition
    const comparisonBases = extractComparisonBases(j, cond)
    if (comparisonBases.length === 0) return null

    const comparisonBase = comparisonBases.find(base => countBaseVariableUsedInAllBranches(j, base, tree) >= SWITCH_THRESHOLD)
    if (!comparisonBase) return null

    const switchCases = collectSwitchCase(j, tree, comparisonBase, true)
    return [j.switchStatement(comparisonBase, switchCases)]
}

function collectSwitchCase(j: JSCodeshift, tree: DecisionTree, base: ExpressionKind, isReturn: boolean): SwitchCase[] {
    const switchCases: SwitchCase[] = []
    const { condition, trueBranch, falseBranch } = tree

    if (!trueBranch && !falseBranch) return switchCases

    const comparisonValues = extractComparisonValues(j, condition, base)
    const [lastComparisonValue, ...otherComparisonValues] = comparisonValues.reverse()
    otherComparisonValues.reverse()

    otherComparisonValues.forEach((comparisonValue) => {
        switchCases.push(j.switchCase(comparisonValue, []))
    })

    if (lastComparisonValue) {
        if (trueBranch) {
            switchCases.push(j.switchCase(
                lastComparisonValue,
                isReturn
                    ? [j.returnStatement(trueBranch.condition)]
                    : [
                            j.expressionStatement(trueBranch.condition),
                            j.breakStatement(),
                        ],
            ))
        }
        else {
            switchCases.push(j.switchCase(lastComparisonValue, []))
        }
    }

    if (falseBranch) {
        if ((falseBranch.trueBranch || falseBranch.falseBranch)) {
            switchCases.push(...collectSwitchCase(j, falseBranch, base, isReturn))
        }
        else {
            switchCases.push(j.switchCase(
                null,
                isReturn
                    ? [j.returnStatement(falseBranch.condition)]
                    : [
                            j.expressionStatement(falseBranch.condition),
                            j.breakStatement(),
                        ]))
        }
    }

    return switchCases
}

/**
 * Find out the base expression of a comparison expression.
 * `a === 1` will return `a`.
 * `a === 1 && a === 2` will return `[]` as they are not the same.
 * `a === 1 || a === 2` will return `a`.
 * `a === 1 || b === 1` will return `[]` as they are not the same.
 * `a == b` will return `[a, b]` as we can't be sure which one is the base.
 * `a == b || a == c` will return `[a]`
 */
function extractComparisonBases(j: JSCodeshift, condition: ExpressionKind): ExpressionKind[] {
    if (j.BinaryExpression.check(condition) && (condition.operator === '==' || condition.operator === '===')) {
        return [condition.left, condition.right].filter(node => isComparisonBase(j, node))
    }

    if (j.LogicalExpression.check(condition) && condition.operator === '||') {
        const leftBases = extractComparisonBases(j, condition.left)
        const rightBases = extractComparisonBases(j, condition.right)

        if (leftBases.length > 0 && rightBases.length > 0) {
            const baseVariable = leftBases.find(node1 => rightBases.find(node2 => areNodesEqual(j, node1, node2)))
            return baseVariable ? [baseVariable] : []
        }
    }

    return []
}

function extractComparisonValues(j: JSCodeshift, node: ExpressionKind, base: ExpressionKind): ExpressionKind[] {
    if (j.BinaryExpression.check(node) && (node.operator === '==' || node.operator === '===')) {
        if (areNodesEqual(j, node.left, base)) return [node.right]
        if (areNodesEqual(j, node.right, base)) return [node.left]
    }

    if (j.LogicalExpression.check(node) && node.operator === '||') {
        const leftValues = extractComparisonValues(j, node.left, base)
        const rightValues = extractComparisonValues(j, node.right, base)
        return [...leftValues, ...rightValues]
    }

    return []
}

function countBaseVariableUsedInAllBranches(j: JSCodeshift, base: ASTNode, tree: DecisionTree, count = 0): number {
    const { condition, trueBranch, falseBranch } = tree

    if (!trueBranch && !falseBranch) return count

    const comparisonBases = extractComparisonBases(j, condition)
    if (!comparisonBases.some(node => areNodesEqual(j, node, base))) return -1

    if (trueBranch && falseBranch) {
        return Math.max(
            countBaseVariableUsedInAllBranches(j, base, trueBranch, count + 1),
            countBaseVariableUsedInAllBranches(j, base, falseBranch, count + 1),
        )
    }

    if (trueBranch) return countBaseVariableUsedInAllBranches(j, base, trueBranch, count + 1)
    if (falseBranch) return countBaseVariableUsedInAllBranches(j, base, falseBranch, count + 1)

    return count
}

function isComparisonBase(j: JSCodeshift, node: ASTNode): boolean {
    if (j.Literal.check(node)) return false
    // move function call to switch can break the semantics
    // as it will only be called once
    if (j.CallExpression.check(node)) return false

    return true
}

export default wrap(transformAST)
