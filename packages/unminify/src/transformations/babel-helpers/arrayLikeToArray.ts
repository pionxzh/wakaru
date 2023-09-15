import { findModuleFromSource } from '../../utils/import'
import { removeDeclarationIfUnused, removeDefaultImportIfUnused } from '../../utils/scope'
import wrap from '../../wrapAstTransformation'
import type { ASTTransformation } from '../../wrapAstTransformation'
import type { Identifier } from '@babel/types'
import type { ArrayExpression, CallExpression, ImportDefaultSpecifier } from 'jscodeshift'

/**
 * `@babel/runtime/helpers/arrayLikeToArray` helper.
 *
 * Replace `empty slot` with `undefined` in ArrayExpression.
 *
 * Note: Semantically, this is not the same as what `arrayWithoutHoles`
 * does, but currently we don't see other usage of `arrayLikeToArray`.
 *
 * We can further optimize this by detecting if we are wrapped by `toConsumableArray`
 * and skip the replacement as spread operator will handle `empty` correctly.
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    const moduleName = '@babel/runtime/helpers/arrayLikeToArray'
    const moduleEsmName = '@babel/runtime/helpers/esm/arrayLikeToArray'
    const moduleSource = findModuleFromSource(j, root, moduleName) || findModuleFromSource(j, root, moduleEsmName)

    if (moduleSource) {
        const isImport = j.ImportDeclaration.check(moduleSource)
        const moduleVariableName = isImport
            ? ((moduleSource.specifiers![0] as ImportDefaultSpecifier).local as Identifier).name
            : (moduleSource.id as Identifier).name

        // arrayLikeToArray([...])
        // arrayLikeToArray.default([...])
        root
            .find(j.CallExpression, {
                callee: (callee: CallExpression['callee']) => {
                    return (
                        j.Identifier.check(callee)
                     && callee.name === moduleVariableName
                    )
                    || (
                        j.MemberExpression.check(callee)
                     && j.Identifier.check(callee.object)
                     && callee.object.name === moduleVariableName
                     && j.Identifier.check(callee.property)
                     && callee.property.name === 'default'
                    )
                },
                arguments: [
                    { type: 'ArrayExpression' } as const,
                ],
            })
            .forEach((path) => {
                const arr = path.node.arguments[0] as ArrayExpression
                const elements = arr.elements.map(element => element ?? j.identifier('undefined'))
                path.replace(j.arrayExpression(elements))

                isImport
                    ? removeDefaultImportIfUnused(j, root, moduleVariableName)
                    : removeDeclarationIfUnused(j, path, moduleVariableName)
            })

        // (0, arrayLikeToArray)([...])
        // (0, arrayLikeToArray.default)([...])
        root
            .find(j.CallExpression, {
                callee: {
                    type: 'SequenceExpression',
                    expressions: [
                        { type: 'Literal', value: 0 },
                        (expression: any) => {
                            return (
                                j.Identifier.check(expression)
                             && expression.name === moduleVariableName
                            )
                            || (
                                j.MemberExpression.check(expression)
                             && j.Identifier.check(expression.object)
                             && expression.object.name === moduleVariableName
                             && j.Identifier.check(expression.property)
                             && expression.property.name === 'default'
                            )
                        },
                    ],
                },
                arguments: [
                    { type: 'ArrayExpression' } as const,
                ],
            })
            .forEach((path) => {
                const arr = path.node.arguments[0] as ArrayExpression
                const elements = arr.elements.map(element => element ?? j.identifier('undefined'))
                path.replace(j.arrayExpression(elements))

                isImport
                    ? removeDefaultImportIfUnused(j, root, moduleVariableName)
                    : removeDeclarationIfUnused(j, path, moduleVariableName)
            })
    }
}

export default wrap(transformAST)
