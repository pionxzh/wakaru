import { isTopLevel } from '@wakaru/ast-utils'
import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import type { ASTTransformation, Context } from '@wakaru/shared/rule'
import type { Scope } from 'ast-types/lib/scope'
import type { ASTPath, Collection, ExportNamedDeclaration, ExportSpecifier, Identifier, JSCodeshift, VariableDeclaration, VariableDeclarator } from 'jscodeshift'

/**
 * @example
 * const a = 1
 * export const b = a
 * ->
 * export const b = 1
 *
 * @example
 * function a() {}
 * export const b = a
 * ->
 * export function b() {}
 *
 * @example
 * class o {}
 * export const App = o
 * ->
 * export class App {}
 *
 * @example
 * const o = class {}
 * export const App = o
 * ->
 * export const App = class {}
 *
 * @example
 * const o = { a: 1 }
 * export { Game as o }
 * ->
 * export const Game = { a: 1 }
 *
 * @example
 * // export default will not be modified
 * const x = <anything>
 * export default x
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    // export const a = <Identifier>
    root
        .find(j.ExportNamedDeclaration, {
            declaration: {
                type: 'VariableDeclaration',
                declarations: [{
                    type: 'VariableDeclarator',
                    id: {
                        type: 'Identifier',
                    },
                    init: {
                        type: 'Identifier',
                    },
                }],

            },
        })
        .forEach((path) => {
            const declaration = path.node.declaration as VariableDeclaration
            const variableDeclarations = declaration.declarations as VariableDeclarator[]

            variableDeclarations.forEach((variableDeclarator) => {
                if (j.Identifier.check(variableDeclarator.id) && j.Identifier.check(variableDeclarator.init)) {
                    const id = variableDeclarator.id as Identifier
                    const init = variableDeclarator.init as Identifier

                    const newName = id.name
                    const oldName = init.name

                    // remove the declarator from the declaration
                    if (variableDeclarations.length === 1) {
                        path.prune()
                    }
                    else {
                        const index = variableDeclarations.indexOf(variableDeclarator)
                        if (index > -1) {
                            variableDeclarations.splice(index, 1)
                        }
                    }

                    inlineExport(context, path, oldName)
                    renameInRoot(j, root, oldName, newName)
                }
            })
        })

    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    // redeclare export
    // export { oldName as newName }
    root
        .find(j.ExportNamedDeclaration, {
            declaration: null,
            specifiers: [{
                type: 'ExportSpecifier',
            }],
        })
        .forEach((path) => {
            const specifiers = path.node.specifiers as ExportSpecifier[]
            if (!(specifiers && j.ExportSpecifier.check(specifiers[0]))) return

            const { local, exported } = specifiers[0]
            if (!j.Identifier.check(local) || !j.Identifier.check(exported)) return

            const newName = exported.name
            const oldName = local.name

            if (rootScope.declares(newName)) return

            inlineExport(context, path, oldName)
            renameInRoot(j, root, oldName, newName)
        })
}

export default createJSCodeshiftTransformationRule({
    name: 'un-export-rename',
    transform: transformAST,
})

function inlineExport(
    context: Context,
    path: ASTPath<ExportNamedDeclaration>,
    name: string,
) {
    const { root, j } = context
    const comments = path.node.comments

    const variableSource = root
        .find(j.VariableDeclaration, {
            declarations: [{
                type: 'VariableDeclarator',
                id: {
                    type: 'Identifier',
                    name,
                },
            }],
        })
        .filter(path => isTopLevel(j, path))

    const functionSource = root
        .find(j.FunctionDeclaration, {
            id: {
                type: 'Identifier',
                name,
            },
        })
        .filter(path => isTopLevel(j, path))

    const classSource = root
        .find(j.ClassDeclaration, {
            id: {
                type: 'Identifier',
                name,
            },
        })
        .filter(path => isTopLevel(j, path))

    if (variableSource.length === 1) {
        const node = variableSource.get().node as VariableDeclaration
        const kind = node.kind
        const declarations = node.declarations as VariableDeclarator[]
        const sourceDeclarator = declarations.find(declarator => j.Identifier.check(declarator.id) && declarator.id.name === name)!

        const { init: sourceInit } = sourceDeclarator

        // wrap the declaration with the export
        const variableDeclarator = j.variableDeclarator(
            j.identifier(name),
            sourceInit,
        )
        const exportNamedDeclaration = j.exportNamedDeclaration(
            j.variableDeclaration(kind, [variableDeclarator]),
        )
        exportNamedDeclaration.comments = comments
        variableSource.replaceWith(exportNamedDeclaration)
    }
    else if (functionSource.length === 1) {
        const sourceDeclaration = functionSource.get().node
        if (!j.FunctionDeclaration.check(sourceDeclaration)) return

        const { id: sourceId, params, body, generator, async } = sourceDeclaration
        if (!j.Identifier.check(sourceId)) return

        // wrap the declaration with the export
        const exportNamedDeclaration = j.exportNamedDeclaration(
            j.functionDeclaration(
                j.identifier(name),
                params,
                body,
                generator,
                async,
            ),
        )
        exportNamedDeclaration.comments = comments
        functionSource.replaceWith(exportNamedDeclaration)
    }
    else if (classSource.length === 1) {
        const sourceDeclaration = classSource.get().node
        if (!j.ClassDeclaration.check(sourceDeclaration)) return

        const { id: sourceId, body, superClass } = sourceDeclaration
        if (!j.Identifier.check(sourceId)) return

        // wrap the declaration with the export
        const exportNamedDeclaration = j.exportNamedDeclaration(
            j.classDeclaration(
                j.identifier(name),
                body,
                superClass,
            ),
        )
        exportNamedDeclaration.comments = comments
        classSource.replaceWith(exportNamedDeclaration)
    }
}

function renameInRoot(j: JSCodeshift, root: Collection, oldName: string, newName: string) {
    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope || !rootScope.declares(oldName) || rootScope.declares(newName)) return

    rootScope.rename(oldName, newName)
    rootScope.markAsStale()
}
