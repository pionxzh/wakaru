import type { ASTPath, JSCodeshift, Node } from 'jscodeshift'

export function isTopLevel(j: JSCodeshift, node: ASTPath<Node>): boolean {
    return j.Program.check(node.parentPath.node)
}
