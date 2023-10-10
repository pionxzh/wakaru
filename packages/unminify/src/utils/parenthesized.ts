import type { ASTNode } from 'ast-types'
import type { JSCodeshift } from 'jscodeshift'

export function getParenthesized(node: any): boolean {
    return !!node?.extra?.parenthesized
}

export function markParenthesized<T extends ASTNode = any>(node: T, value: boolean): T {
    const n = node as any
    if (!('extra' in n)) {
        n.extra = {}
    }
    n.extra.parenthesized = value

    return node
}

export function shouldParenthesized(j: JSCodeshift, node: ASTNode) {
    return !j.Identifier.check(node)
        && !j.StringLiteral.check(node)
        && !j.NumericLiteral.check(node)
        && !j.MemberExpression.check(node)
        && !j.CallExpression.check(node)
        && !j.OptionalCallExpression.check(node)
}

export function smartParenthesized<T extends ASTNode = any>(j: JSCodeshift, node: T) {
    return markParenthesized(node, shouldParenthesized(j, node))
}
