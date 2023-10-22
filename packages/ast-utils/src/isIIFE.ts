import { fromPaths } from 'jscodeshift/src/Collection'
import type { ASTPath, CallExpression, Collection, ExpressionStatement, JSCodeshift, Statement } from 'jscodeshift'

/**
 * @example
 * ```js
 * (() => { ... })(...)
 * (function() { ... })(...)
 * ```
 *
 * @example
 * ```js
 * !function() { ... }(...)
 * ```
 */
export function isIIFE(j: JSCodeshift, node: Statement): node is ExpressionStatement {
    if (!j.ExpressionStatement.check(node)) return false

    const expression = node.expression
    if (j.CallExpression.check(expression)) {
        return j.FunctionExpression.check(expression.callee)
            || j.ArrowFunctionExpression.check(expression.callee)
    }

    if (j.UnaryExpression.check(expression) && expression.operator === '!') {
        return j.FunctionExpression.check(expression.argument)
    }

    return false
}

export function queryIIFE(j: JSCodeshift, collection: Collection): Collection<CallExpression> {
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
        .map(path => (path.get('expression') as ASTPath<CallExpression>))
        .paths()

    const collection2 = collection
        .find(j.ExpressionStatement, {
            expression: {
                type: 'UnaryExpression',
                operator: '!',
                argument: {
                    type: 'CallExpression',
                },
            },
        })
        .map(path => (path.get('expression', 'argument') as ASTPath<CallExpression>))
        .paths()

    return fromPaths([...collection1, ...collection2], collection)
}
