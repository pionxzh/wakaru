import type { ASTNode } from 'jscodeshift'

export interface NodePosition {
    start: number
    end: number
}

export function getNodePosition(node: ASTNode): NodePosition | null {
    if (
        'start' in node && typeof node.start === 'number'
     && 'end' in node && typeof node.end === 'number'
    ) {
        return {
            start: node.start,
            end: node.end,
        }
    }

    // no position info, means it's a newly inserted node
    return null
}
