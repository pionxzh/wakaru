import { findModuleFromSource } from '../../utils/import'
import { removeDeclarationIfUnused, removeDefaultImportIfUnused } from '../../utils/scope'
import wrap from '../../wrapAstTransformation'
import type { ASTTransformation } from '../../wrapAstTransformation'
import type { Identifier } from '@babel/types'
import type { ASTPath, CallExpression, Collection, ImportDefaultSpecifier, JSCodeshift, ObjectExpression } from 'jscodeshift'

/**
 * Restore object spread syntax from `@babel/runtime/helpers/objectSpread2` helper.
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    /**
     * `objectSpread2` was introduced in Babel v7.5.0
     */
    const moduleName = '@babel/runtime/helpers/objectSpread2'
    const moduleEsmName = '@babel/runtime/helpers/esm/objectSpread2'
    const fallbackModuleName = '@babel/runtime/helpers/objectSpread'
    const fallbackModuleEsmName = '@babel/runtime/helpers/esm/objectSpread'
    const moduleSource = findModuleFromSource(j, root, moduleName)
    || findModuleFromSource(j, root, moduleEsmName)
    || findModuleFromSource(j, root, fallbackModuleName)
    || findModuleFromSource(j, root, fallbackModuleEsmName)

    if (moduleSource) {
        const isImport = j.ImportDeclaration.check(moduleSource)
        const moduleVariableName = isImport
            ? ((moduleSource.specifiers![0] as ImportDefaultSpecifier).local as Identifier).name
            : (moduleSource.id as Identifier).name

        // objectSpread({}, foo)
        // objectSpread.default({ x }, y)
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
            })
            .paths()
            .reverse()
            .forEach((path) => {
                handleSpread(j, root, path, isImport, moduleVariableName)
            })

        // (0, objectSpread)([...])
        // (0, objectSpread.default)([...])
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
            })
            .paths()
            .reverse()
            .forEach((path) => {
                handleSpread(j, root, path, isImport, moduleVariableName)
            })
    }
}

function handleSpread(j: JSCodeshift, root: Collection, path: ASTPath<CallExpression>, isImport: boolean, moduleVariableName: string) {
    const properties: ObjectExpression['properties'] = []

    for (const arg of path.node.arguments) {
        if (j.ObjectExpression.check(arg)) {
            properties.push(...arg.properties)
        }
        else if (j.SpreadElement.check(arg)) {
            properties.push(arg)
        }
        else {
            properties.push(j.spreadElement(arg))
        }
    }

    const spreadObject = j.objectExpression(properties)
    path.replace(spreadObject)

    isImport
        ? removeDefaultImportIfUnused(j, root, moduleVariableName)
        : removeDeclarationIfUnused(j, path, moduleVariableName)
}

export default wrap(transformAST)
