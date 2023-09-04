import type { ASTNode, JSCodeshift } from 'jscodeshift'

export function areNodesEqual(j: JSCodeshift, node1: ASTNode, node2: ASTNode): boolean {
    return j(node1).toSource() === j(node2).toSource()
}
