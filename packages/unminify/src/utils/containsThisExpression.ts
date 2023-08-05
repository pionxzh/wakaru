import j from 'jscodeshift'
import type { ASTNode } from 'jscodeshift'

export function containsThisExpression(node: ASTNode): boolean {
    return j(node).find(j.ThisExpression).size() > 0
}
