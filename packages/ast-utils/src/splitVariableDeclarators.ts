import type { Collection, JSCodeshift } from 'jscodeshift'

/**
 * ```js
 * var a = 1, b = true, c = func(d)
 * ->
 * var a = 1
 * var b = true
 * var c = func(d)
 * ```
 */
export function splitVariableDeclarators(j: JSCodeshift, collection: Collection) {
    collection
        .find(j.VariableDeclaration, {
            declarations: [
                {
                    type: 'VariableDeclarator',
                    id: { type: 'Identifier' },
                },
            ],
        })
        .filter((path) => {
            if (path.parent?.node.type === 'ForStatement') return false
            return path.node.declarations.length > 1
        })
        .forEach((p) => {
            const { kind, declarations } = p.node
            j(p).replaceWith(declarations.map(d => j.variableDeclaration(kind, [d])))
        })
}
