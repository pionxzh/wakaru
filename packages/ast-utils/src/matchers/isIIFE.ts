import { fromPaths } from 'jscodeshift/src/Collection'
import type { ASTNode, ASTPath, CallExpression, Collection, ExpressionStatement, JSCodeshift, Statement } from 'jscodeshift'

/**
 * @example
 * ```js
 * (() => { ... })(...)
 * (function() { ... })(...)
 * !(() => { ... })(...)
 * !function() { ... }(...)
 * ```
 */
export function isStatementIIFE(j: JSCodeshift, node: Statement): node is ExpressionStatement {
    if (!j.ExpressionStatement.check(node)) return false

    const expression = node.expression
    return isIIFE(j, expression)
}

/**
 * @example
 * ```js
 * (() => { ... })(...)
 * (function() { ... })(...)
 * !(() => { ... })(...)
 * !function() { ... }(...)
 * ```
 */
export function isIIFE(j: JSCodeshift, node: ASTNode): node is ExpressionStatement {
    if (j.UnaryExpression.check(node) && node.operator === '!') {
        node = node.argument
    }

    if (j.CallExpression.check(node)) {
        return j.FunctionExpression.check(node.callee)
            || j.ArrowFunctionExpression.check(node.callee)
    }

    return false
}

export function findIIFEs(
    j: JSCodeshift,
    collection: Collection,
    additionalFilter?: (path: ASTPath) => boolean,
): Collection<CallExpression> {
    const collection1 = collection
        .find(j.ExpressionStatement, {
            expression: {
                type: 'CallExpression',
                callee: {
                    type: (type: string) => {
                        return type === 'FunctionExpression'
                            || type === 'ArrowFunctionExpression'
                    },
                },
            },
        })
        .filter(path => additionalFilter ? additionalFilter(path) : true)
        .map(path => (path.get('expression') as ASTPath<CallExpression>))
        .paths()

    const collection2 = collection
        .find(j.ExpressionStatement, {
            expression: {
                type: 'UnaryExpression',
                operator: '!',
                argument: {
                    type: 'CallExpression',
                    callee: {
                        type: (type: string) => {
                            return type === 'FunctionExpression'
                                || type === 'ArrowFunctionExpression'
                        },
                    },
                },
            },
        })
        .filter(path => additionalFilter ? additionalFilter(path) : true)
        .map(path => (path.get('expression', 'argument') as ASTPath<CallExpression>))
        .paths()

    return fromPaths([...collection1, ...collection2])
}
