import type { ConditionalExpression, Identifier, Literal, LogicalExpression } from 'jscodeshift'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

interface Case { test: Literal | null; consequent: any }

/**
 * Un-minify Switch case
 *
 * This is a really special case.
 * I'm not sure which minifier does this kind of crazy thing.
 *
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
 * Another example without default:
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
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    function getIdentifierLiteral(left: any, right: any, identifierName?: string): [Identifier, Literal] | null {
        if (j.Identifier.check(left) && j.Literal.check(right) && (!identifierName || left.name === identifierName)) {
            return [left, right]
        }
        if (j.Identifier.check(right) && j.Literal.check(left) && (!identifierName || right.name === identifierName)) {
            return [right, left]
        }
        return null
    }

    function dfsLogicalExpression(
        node: LogicalExpression,
        subCases: Case[],
        identifierName: string,
    ): boolean {
        const { left, right, operator } = node
        if (operator === '||') {
            if (j.LogicalExpression.check(left)) {
                dfsLogicalExpression(left, subCases, identifierName)
            }
            else if (j.BinaryExpression.check(left)) {
                const identifierLiteral = getIdentifierLiteral(left.left, left.right, identifierName)
                if (!identifierLiteral) {
                    return false
                }
                const [_, _literal] = identifierLiteral
                subCases.push({
                    test: _literal,
                    consequent: null,
                })
            }
            else {
                return false
            }

            if (j.LogicalExpression.check(right)) {
                dfsLogicalExpression(right, subCases, identifierName)
            }
            else if (j.BinaryExpression.check(right)) {
                const identifierLiteral = getIdentifierLiteral(right.left, right.right, identifierName)
                if (!identifierLiteral) {
                    return false
                }
                const [_, _literal] = identifierLiteral
                subCases.push({
                    test: _literal,
                    consequent: null,
                })
            }
            else {
                return false
            }
        }
        else if (operator === '&&') {
            if (j.LogicalExpression.check(left)) {
                dfsLogicalExpression(left, subCases, identifierName)
            }
            else if (j.BinaryExpression.check(left)) {
                const identifierLiteral = getIdentifierLiteral(left.left, left.right, identifierName)
                if (!identifierLiteral) {
                    return false
                }
                const [_, _literal] = identifierLiteral
                subCases.push({
                    test: _literal,
                    consequent: right,
                })
            }
            else {
                return false
            }
        }

        return true
    }

    root
        .find(j.ConditionalExpression, {
            test: {
                type: 'BinaryExpression',
                operator: '==',
            },
            alternate: {
                type: 'ConditionalExpression',
            },
        })
        .forEach((path) => {
            const { node } = path
            const { test, alternate } = node
            if (!j.BinaryExpression.check(test) || !j.ConditionalExpression.check(alternate)) return

            const { left, right } = test
            const identifierLiteral = getIdentifierLiteral(left, right)
            if (!identifierLiteral) return

            const [identifier] = identifierLiteral
            const cases: Case[] = []

            let isSwitch = true
            let current: ConditionalExpression = node

            while (current) {
                const { test, consequent, alternate } = current
                if (j.BinaryExpression.check(test)) {
                    const { operator, left, right } = test
                    if (operator !== '==') {
                        isSwitch = false
                        break
                    }
                    const identifierLiteral = getIdentifierLiteral(left, right, identifier.name)
                    if (!identifierLiteral) {
                        isSwitch = false
                        break
                    }
                    const [_identifier, _literal] = identifierLiteral

                    cases.push({
                        test: _literal,
                        consequent,
                    })

                    if (j.ConditionalExpression.check(alternate)) {
                        current = alternate
                    }
                    // process the last part
                    else if (j.LogicalExpression.check(alternate)) {
                        const logicalExpression = alternate

                        // dfs to collect all the cases
                        const subCases: Case[] = []
                        const result = dfsLogicalExpression(logicalExpression, subCases, identifier.name)
                        if (!result) {
                            isSwitch = false
                            break
                        }
                        if (subCases.length > 0) {
                            cases.push(...subCases)
                        }
                        break
                    }
                    else {
                        // default
                        cases.push({
                            test: null,
                            consequent: alternate,
                        })
                        break
                    }
                }
                else if (j.LogicalExpression.check(test)) {
                    const logicalExpression = test

                    // dfs to collect all the cases
                    const subCases: Case[] = []
                    const dfs = (node: LogicalExpression) => {
                        const { left, right, operator } = node
                        if (operator !== '||') {
                            isSwitch = false
                            return
                        }

                        if (j.LogicalExpression.check(left)) {
                            dfs(left)
                        }
                        else if (j.BinaryExpression.check(left)) {
                            const identifierLiteral = getIdentifierLiteral(left.left, left.right, identifier.name)
                            if (!identifierLiteral) {
                                isSwitch = false
                                return
                            }
                            const [_, _literal] = identifierLiteral
                            subCases.push({
                                test: _literal,
                                consequent: null,
                            })
                        }
                        else {
                            isSwitch = false
                            return
                        }

                        if (j.LogicalExpression.check(right)) {
                            dfs(right)
                        }
                        else if (j.BinaryExpression.check(right)) {
                            const identifierLiteral = getIdentifierLiteral(right.left, right.right, identifier.name)
                            if (!identifierLiteral) {
                                isSwitch = false
                                return
                            }
                            const [_, _literal] = identifierLiteral
                            subCases.push({
                                test: _literal,
                                consequent: null,
                            })
                        }
                        else {
                            isSwitch = false
                        }
                    }
                    dfs(logicalExpression)
                    if (subCases.length === 0) break
                    subCases[subCases.length - 1].consequent = consequent
                    cases.push(...subCases)

                    if (j.ConditionalExpression.check(alternate)) {
                        current = alternate
                    }
                    else if (j.LogicalExpression.check(alternate)) {
                        const logicalExpression = alternate

                        // dfs to collect all the cases
                        const subCases: Case[] = []
                        const result = dfsLogicalExpression(logicalExpression, subCases, identifier.name)
                        if (!result) {
                            isSwitch = false
                            break
                        }
                        if (subCases.length > 0) {
                            cases.push(...subCases)
                        }
                    }
                    else {
                        cases.push({
                            test: null,
                            consequent: alternate,
                        })
                        break
                    }
                }
                else {
                    isSwitch = false
                    break
                }
            }

            if (isSwitch) {
                const switchCases = cases.map((c) => {
                    const statement = c.consequent ? [j.blockStatement([j.expressionStatement(c.consequent), j.breakStatement()])] : []
                    return j.switchCase(c.test, statement)
                })
                const switchStatement = j.switchStatement(identifier, switchCases)
                path.replace(switchStatement)
            }
        })
}

export default wrap(transformAST)
