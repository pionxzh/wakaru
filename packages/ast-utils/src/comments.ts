import { getNodePosition } from './position'
import type { CommentKind } from 'ast-types/lib/gen/kinds'
import type { Collection, JSCodeshift, Node } from 'jscodeshift'

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
        const posA = getNodePosition(a)
        const posB = getNodePosition(b)
        return (posA?.start ?? 0) - (posB?.start ?? 0)
    })
}

export function pruneComments(j: JSCodeshift, collection: Collection): void {
    // @ts-expect-error - Comment type is wrong
    collection.find(j.Comment).forEach(path => path.prune())
}

export function removePureAnnotation(j: JSCodeshift, node: Node) {
    const comments = node.comments || []
    node.comments = comments.filter(c => !isPureAnnotation(j, c))

    return node
}

// /*#__PURE__*/
function isPureAnnotation(j: JSCodeshift, comment: CommentKind) {
    return j.CommentBlock.check(comment)
    && comment.value.trim() === '#__PURE__'
}
