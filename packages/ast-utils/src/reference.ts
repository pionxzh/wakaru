import type { Scope } from 'ast-types/lib/scope'
import type { ASTNode, ASTPath, Collection, Identifier, JSCodeshift } from 'jscodeshift'

/**
 * Checks if the identifier is a variable name.
 *
 * Based on jscodeshift's implementation.
 * @see https://github.com/facebook/jscodeshift/blob/c2ba556b6233067c61d83e2913ba7557881655a1/src/collections/VariableDeclarator.js#L79
 */
export function isVariableIdentifier(j: JSCodeshift, path: ASTPath<Identifier>): boolean {
    const parent = path.parent.node

    if (
        j.FunctionExpression.check(parent)
        && parent.id === path.node
    ) {
        // var a = function oldName() {}
        return false
    }

    if (
        j.MemberExpression.check(parent)
        && parent.property === path.node
        && !parent.computed
    ) {
        // obj.oldName
        return false
    }

    if (
        j.Property.check(parent)
        && parent.key === path.node
        && !parent.computed
    ) {
        // { oldName: 3 }
        return false
    }

    if (
        j.ObjectProperty.check(parent)
        && parent.key === path.node
        && !parent.computed
    ) {
        // { oldName: 3 }
        return false
    }

    if (
        j.ObjectMethod.check(parent)
        && parent.key === path.node
        && !parent.computed
    ) {
        // { oldName() {} }
        return false
    }

    if (
        j.MethodDefinition.check(parent)
        && parent.key === path.node
        && !parent.computed
    ) {
        // class A { oldName() {} }
        return false
    }

    if (
        j.ClassMethod.check(parent)
        && parent.key === path.node
        && !parent.computed
    ) {
        // class A { oldName() {} }
        return false
    }

    if (
        j.ClassProperty.check(parent)
        && parent.key === path.node
        && !parent.computed
    ) {
        // class A { oldName = 3 }
        return false
    }

    if (
        j.JSXAttribute.check(parent)
        // @ts-expect-error
        && parent.name === path.node
        // @ts-expect-error
        && !parent.computed
    ) {
        // <Foo oldName={oldName} />
        return false
    }

    if (
        j.LabeledStatement.check(parent)
        && parent.label === path.node
    ) {
        // oldName: ...
        return false
    }

    if (j.ContinueStatement.check(parent)) {
        // continue oldName
        return false
    }

    if (j.BreakStatement.check(parent)) {
        // break oldName
        return false
    }

    return true
}

export function findReferences(
    j: JSCodeshift,
    nodeOrScope: Scope | ASTNode,
    identifierName: string,
): Collection<Identifier> {
    const targetScope = 'bindings' in nodeOrScope ? nodeOrScope : j(nodeOrScope).get().scope as Scope
    const range = 'bindings' in nodeOrScope ? nodeOrScope.path : nodeOrScope

    return j(range)
        .find(j.Identifier, { name: identifierName })
        .filter(path => isVariableIdentifier(j, path))
        .filter((path) => {
            // ignore properties (e.g. in MemberExpression
            if (path.name === 'property') return false

            if (!path.scope) return false
            let scope = path.scope
            // we don't use `scope.lookup` here to avoid
            // traversing the whole scope chain
            while (scope && scope !== targetScope) {
                if (scope.declares(identifierName)) {
                    return false // identifier is shadowed
                }
                scope = scope.parent
            }

            return !!scope
        })
}

export function renameIdentifier(
    j: JSCodeshift,
    targetScope: Scope,
    oldName: string,
    newName: string,
): void {
    if (oldName === newName) return

    const references = findReferences(j, targetScope, oldName)
    references.forEach((path) => {
        path.node.name = newName
    })

    // mark the scope as stale to trigger a re-scan in the next lookup
    targetScope.markAsStale()
}
