import type { ASTPath, JSCodeshift, Node } from 'jscodeshift'

export function isTopLevel(j: JSCodeshift, path: ASTPath<Node>): boolean {
    return j.Program.check(path.parentPath.node)
}
