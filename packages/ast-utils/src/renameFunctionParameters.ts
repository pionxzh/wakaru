import type { Scope } from 'ast-types/lib/scope'
import type { ArrowFunctionExpression, FunctionDeclaration, FunctionExpression, JSCodeshift } from 'jscodeshift'

export function renameFunctionParameters(j: JSCodeshift, node: FunctionDeclaration | FunctionExpression | ArrowFunctionExpression, parameters: string[]): void {
    if (
        !j.FunctionDeclaration.check(node)
        && !j.FunctionExpression.check(node)
        && !j.ArrowFunctionExpression.check(node)
    ) return

    node.params.forEach((param, index) => {
        if (param.type === 'Identifier') {
            const oldName = param.name
            const newName = parameters[index]
            if (!newName || oldName === newName) return

            const functionScope = j(node).closestScope().get()

            j(functionScope)
                .find(j.Identifier, { name: oldName })
                // ref: https://github.com/facebook/jscodeshift/blob/c2ba556b6233067c61d83e2913ba7557881655a1/src/collections/VariableDeclarator.js#L79
                .filter((path) => { // ignore non-variables
                    const parent = path.parent.node

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
                })
                .forEach((path) => {
                    const pathScope = path.scope.lookup(oldName) as Scope
                    const scopeNode = pathScope.getBindings()[oldName]?.[0].scope.node
                    if (scopeNode === functionScope.value && path.name !== 'property') {
                        path.node.name = newName
                    }
                })
        }
    })
}
