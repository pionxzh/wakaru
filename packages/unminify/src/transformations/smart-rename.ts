import { renameIdentifier } from '@wakaru/ast-utils'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { Collection, JSCodeshift, ObjectPattern } from 'jscodeshift'

/**
 * Converts object property accesses and array index accesses to destructuring.
 *
 * @example
 * let { gql: t, dispatchers: o, listener: i } = n;
 * o.delete(t, i);
 * ->
 * let { gql, dispatchers, listener } = n;
 * dispatchers.delete(mql, listener);
 *
 * @TODO
 * const I = e.atom,
 * export default {
 *   themeAtom: I,
 * };
 * ->
 * const themeAtom = e.atom,
 * export default {
 *  themeAtom,
 * };
 *
 * @TODO
 * const d = o.createContext(u);
 * ->
 * const uContext = o.createContext(u);
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    handleDestructuringRename(j, root)
}

/**
 * let { gql: t, dispatchers: o, listener: i } = n;
 * o.delete(t, i);
 * ->
 * let { gql, dispatchers, listener } = n;
 * dispatchers.delete(mql, listener);
 */
function handleDestructuringRename(j: JSCodeshift, root: Collection) {
    root
        .find(j.VariableDeclarator, { id: { type: 'ObjectPattern' } })
        .forEach((path) => {
            const scope = path.scope
            if (!scope) return

            const id = path.node.id as ObjectPattern
            id.properties.forEach((property) => {
                if (!j.ObjectProperty.check(property)) return
                if (property.computed || property.shorthand) return
                const key = property.key
                const value = property.value
                if (!j.Identifier.check(key) || !j.Identifier.check(value)) return

                // If the key is longer than the value, rename the value
                if (key.name.length > value.name.length) {
                    renameIdentifier(j, scope, value.name, key.name)
                    if (key.name === value.name) {
                        property.shorthand = true
                    }
                }
            })
        })
}

export default wrap(transformAST)
