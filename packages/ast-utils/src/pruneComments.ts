import type { Collection, JSCodeshift } from 'jscodeshift'

export function pruneComments(j: JSCodeshift, collection: Collection): void {
    // @ts-expect-error - Comment type is wrong
    collection.find(j.Comment).forEach(path => path.prune())
}
