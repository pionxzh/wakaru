import type { ASTNode, Type } from 'ast-types/types'
import type { ASTPath } from 'jscodeshift'

/**
 * Traverse the AST up and finds the closest node of the provided type.
 *
 * @link https://github.com/facebook/jscodeshift/blob/51da1a5c4ba3707adb84416663634d4fc3141cbb/src/collections/Node.js#L76
 */
export function closest<T extends ASTNode>(path: ASTPath, type: Type<T>): ASTPath<T> | null {
    let parent = path.parent
    while (parent && !(type.check(parent.value))) {
        parent = parent.parent
    }
    return parent || null
}
