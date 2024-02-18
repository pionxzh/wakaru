import { assertScopeExists } from '@wakaru/ast-utils/assert'
import { findReferences } from '@wakaru/ast-utils/reference'
import { isDeclared } from '@wakaru/ast-utils/scope'
import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import type { ASTPath, ClassMethod, FunctionDeclaration, FunctionExpression, JSCodeshift } from 'jscodeshift'

/**
 * Transform `arguments` to `...args` in function parameters.
 *
 * Credits to `lebab` for the original implementation
 *
 * @see https://github.com/lebab/lebab/blob/master/src/transform/argRest.js
 * @see https://babeljs.io/docs/babel-plugin-transform-parameters
 */
export default createJSCodeshiftTransformationRule({
    name: 'un-parameter-rest',
    transform: (context) => {
        const { root, j } = context

        root
            .find(j.FunctionExpression, { params: params => params.length === 0 })
            .forEach(path => handleFunctionArguments(path, j))

        root
            .find(j.FunctionDeclaration, { params: params => params.length === 0 })
            .forEach(path => handleFunctionArguments(path, j))

        root
            .find(j.ClassMethod, { params: params => params.length === 0 })
            .forEach(path => handleFunctionArguments(path, j))
    },
})

/**
 * @example
 * ```ts
 * function foo() {
 *   console.log(arguments);
 * }
 * ```
 * ->
 * ```ts
 * function foo(...args) {
 *  console.log(args);
 * }
 */
function handleFunctionArguments(path: ASTPath<FunctionExpression | FunctionDeclaration | ClassMethod>, j: JSCodeshift) {
    const scope = path.scope
    assertScopeExists(scope)

    if (isDeclared(scope, 'args')) return
    if (isDeclared(scope, 'arguments')) return

    const argumentsReferences = findReferences(j, scope, 'arguments')
    if (argumentsReferences.length === 0) return
    if (argumentsReferences.some((p) => {
        assertScopeExists(p.scope)
        return isDeclared(p.scope, 'args')
    })) return

    argumentsReferences.forEach((p) => {
        p.node.name = 'args'
    })
    path.value.params = [j.restElement(j.identifier('args'))]
    scope.markAsStale()
}
