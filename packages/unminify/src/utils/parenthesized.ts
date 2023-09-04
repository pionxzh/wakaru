export function getParenthesized(node: any): boolean {
    return !!node?.extra?.parenthesized
}

export function markParenthesized<T = any>(node: T, value: boolean): T {
    const n = node as any
    if (!('extra' in n)) {
        n.extra = {}
    }
    n.extra.parenthesized = value

    return node
}
