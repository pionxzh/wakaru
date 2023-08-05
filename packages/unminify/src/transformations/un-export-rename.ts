import { isTopLevel } from '@unminify/ast-utils'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation, Context } from '../wrapAstTransformation'
import type { ASTPath, ExportNamedDeclaration } from 'jscodeshift'

/**
 * const a = 1
 * export const b = a
 * ->
 * export const b = 1
 *
 * function a() {}
 * export const b = a
 * ->
 * export function b() {}
 *
 * class o {}
 * export const App = o
 * ->
 * export class App {}
 *
 * const o = class {}
 * export const App = o
 * ->
 * export const App = class {}
 *
 * const o = { a: 1 }
 * export { Game as o }
 *
 * const x = <anything>
 * export default x
 * ->
 * export default <anything>
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    // export const a = <something>
    root
        .find(j.ExportNamedDeclaration, {
            declaration: {
                type: 'VariableDeclaration',
            },
        })
        .forEach((path) => {
            const { declaration } = path.node

            if (!j.VariableDeclaration.check(declaration)) return
            if (declaration.declarations.length !== 1) return
            const variableDeclaration = declaration.declarations[0]
            if (!j.VariableDeclarator.check(variableDeclaration)) return

            const { id, init } = variableDeclaration
            if (!j.Identifier.check(id) || !j.Identifier.check(init)) return

            const targetName = id.name
            const sourceName = init.name
            const comments = path.node.comments

            console.log(`Inline export ${sourceName} -> ${targetName}`)
            inlineExport(context, path, targetName, sourceName, comments)
        })

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
            const { specifiers } = path.node
            if (!(specifiers && j.ExportSpecifier.check(specifiers[0]))) return

            const { local, exported } = specifiers[0]
            if (!j.Identifier.check(local) || !j.Identifier.check(exported)) return

            const targetName = exported.name
            const sourceName = local.name
            const comments = path.node.comments

            // console.log(`Rename export ${sourceName} -> ${targetName}`)
            inlineExport(context, path, targetName, sourceName, comments)
        })
}

export default wrap(transformAST)

function inlineExport(
    context: Context,
    path: ASTPath<ExportNamedDeclaration>,
    targetName: string,
    sourceName: string,
    comments?: any,
) {
    const { root, j } = context

    const variableSource = root
        .find(j.VariableDeclaration, {
            declarations: [{
                type: 'VariableDeclarator',
                id: {
                    type: 'Identifier',
                    name: sourceName,
                },
            }],
        })
        .filter(path => isTopLevel(j, path))

    const functionSource = root
        .find(j.FunctionDeclaration, {
            id: {
                type: 'Identifier',
                name: sourceName,
            },
        })
        .filter(path => isTopLevel(j, path))

    const classSource = root
        .find(j.ClassDeclaration, {
            id: {
                type: 'Identifier',
                name: sourceName,
            },
        })
        .filter(path => isTopLevel(j, path))

    if (variableSource.length === 1) {
        const node = variableSource.get().node
        const { kind, declarations } = node
        const sourceDeclaration = declarations[0]
        if (!j.VariableDeclarator.check(sourceDeclaration)) return

        const { id: sourceId, init: sourceInit } = sourceDeclaration
        if (!j.Identifier.check(sourceId)) return

        // wrap the declaration with the export
        const variableDeclarator = j.variableDeclarator(
            j.identifier(sourceName),
            sourceInit,
        )
        const exportNamedDeclaration = j.exportNamedDeclaration(
            j.variableDeclaration(kind, [
                variableDeclarator,
            ]),
        )
        exportNamedDeclaration.comments = comments
        variableSource.replaceWith(exportNamedDeclaration)

        renameTopLevelVariableDeclaration(context, sourceName, targetName)
    }
    else if (functionSource.length === 1) {
        const sourceDeclaration = functionSource.get().node
        if (!j.FunctionDeclaration.check(sourceDeclaration)) return

        const { id: sourceId, params, body, generator, async } = sourceDeclaration
        if (!j.Identifier.check(sourceId)) return

        // wrap the declaration with the export
        const exportNamedDeclaration = j.exportNamedDeclaration(
            j.functionDeclaration(
                j.identifier(sourceName),
                params,
                body,
                generator,
                async,
            ),
        )
        exportNamedDeclaration.comments = comments
        functionSource.replaceWith(exportNamedDeclaration)

        renameTopLevelFunction(context, sourceName, targetName)
    }
    else if (classSource.length === 1) {
        const sourceDeclaration = classSource.get().node
        if (!j.ClassDeclaration.check(sourceDeclaration)) return

        const { id: sourceId, body, superClass } = sourceDeclaration
        if (!j.Identifier.check(sourceId)) return

        // wrap the declaration with the export
        const exportNamedDeclaration = j.exportNamedDeclaration(
            j.classDeclaration(
                j.identifier(sourceName),
                body,
                superClass,
            ),
        )
        exportNamedDeclaration.comments = comments
        classSource.replaceWith(exportNamedDeclaration)

        renameClassDeclaration(context, sourceName, targetName)
    }
    if (variableSource.length === 1 || functionSource.length === 1 || classSource.length === 1) {
        // remove the original declaration
        path.prune()
    }
}

function renameTopLevelVariableDeclaration(
    context: Context,
    sourceName: string,
    targetName: string,
) {
    const { root } = context
    // console.log(`Rename variable declaration ${sourceName} -> ${targetName}`)

    root.findVariableDeclarators(sourceName)
        .filter(path => path.scope.isGlobal)
        .renameTo(targetName)
}

function renameTopLevelFunction(
    context: Context,
    sourceName: string,
    targetName: string,
) {
    const { root, j } = context
    const path = root.find(j.FunctionDeclaration, {
        id: {
            type: 'Identifier',
            name: sourceName,
        },
    }).get()
    if (!j.Identifier.check(path.node.id)) return

    const { params, body, generator, async } = path.node

    // transform `function a() {}` to `const a = function() {}`
    const variableDeclarator = j.variableDeclarator(
        j.identifier(sourceName),
        j.functionExpression(
            null,
            params,
            body,
            generator,
            async,
        ),
    )
    const variableDeclaration = j.variableDeclaration('const', [
        variableDeclarator,
    ])
    path.replace(variableDeclaration)

    // rename all references to the function
    root.findVariableDeclarators(sourceName)
        .filter(path => path.scope.isGlobal)
        .renameTo(targetName)

    // transform `const a = function() {}` back to `function a() {}`
    const functionDeclaration = j.functionDeclaration(
        j.identifier(targetName),
        params,
        body,
        generator,
        async,
    )
    console.log(path.node.type)
    path.replace(functionDeclaration)
}

function renameClassDeclaration(
    context: Context,
    sourceName: string,
    targetName: string,
) {
    const { root, j } = context
    const path = root.find(j.ClassDeclaration, {
        id: {
            type: 'Identifier',
            name: sourceName,
        },
    }).get()
    if (!j.Identifier.check(path.node.id)) return

    const { body, superClass } = path.node

    // transform `class a {}` to `const a = class {}`
    const variableDeclarator = j.variableDeclarator(
        j.identifier(sourceName),
        j.classExpression(
            null,
            body,
            superClass,
        ),
    )
    const variableDeclaration = j.variableDeclaration('const', [
        variableDeclarator,
    ])
    path.replace(variableDeclaration)

    // rename all references to the class
    root.findVariableDeclarators(sourceName)
        .filter(path => path.scope.isGlobal)
        .renameTo(targetName)

    // transform `const a = class {}` back to `class a {}`
    const classDeclaration = j.classDeclaration(
        j.identifier(targetName),
        body,
        superClass,
    )
    path.replace(classDeclaration)
}
