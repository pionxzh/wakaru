import type { CommentKind } from 'ast-types/lib/gen/kinds'
import type { Node } from 'jscodeshift'

export function mergeComments(node: Node | Node[], commentsToMerge: CommentKind[] | null | undefined) {
    if (!commentsToMerge) return

    if (!Array.isArray(node)) {
        const comments = node.comments || []
        node.comments = sortComments([...comments, ...commentsToMerge])
    }
    else {
        const leadingComments = commentsToMerge.filter(c => c.leading)
        const nonLeading = commentsToMerge.filter(c => !c.leading)

        if (leadingComments.length > 0) {
            const firstNode = node[0]
            const comments = firstNode.comments || []
            firstNode.comments = sortComments([...comments, ...leadingComments])
        }

        if (nonLeading.length > 0) {
            const lastNode = node[node.length - 1]
            const comments = lastNode.comments || []
            lastNode.comments = sortComments([...comments, ...nonLeading])
        }
    }
}

function sortComments(comments: CommentKind[]) {
    return comments.sort((a, b) => {
        // @ts-expect-error - start is not defined in the type
        const startA = a.start || 0
        // @ts-expect-error - start is not defined in the type
        const startB = b.start || 0
        return startA - startB
    })
}
