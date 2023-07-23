import type { ASTNode } from 'jscodeshift'
import j from 'jscodeshift'

export function containsThisExpression(node: ASTNode): boolean {
    return j(node).find(j.ThisExpression).size() > 0
}
